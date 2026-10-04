//! Independent physical timing checks for offline HRIR rate conversion.

// Rust guideline compliant 2026-02-21
use sotf_host::sofa::{SofaFile, SourcePosition};
use sotf_plugin_binaural::hrtf::resample_sofa;

fn impulses(rate: u32, length: usize, positions: &[usize]) -> SofaFile {
    let mut samples = vec![0.0; positions.len() * 2 * length];
    for (measurement, &position) in positions.iter().enumerate() {
        samples[measurement * 2 * length + position] = 1.0;
        samples[(measurement * 2 + 1) * length + position] = -0.5;
    }
    SofaFile {
        sample_rate: f64::from(rate),
        num_measurements: positions.len(),
        ir_length: length,
        positions: positions
            .iter()
            .enumerate()
            .map(|(index, _)| SourcePosition::new(index as f32 * 15.0, 0.0, 1.0))
            .collect(),
        impulse_responses: samples,
        convention: "SimpleFreeFieldHRIR".into(),
        data_sample_rate: Some(f64::from(rate)),
    }
}

#[test]
fn first_and_last_impulses_preserve_physical_time_across_rates() {
    for (source_rate, target_rate) in [
        (48_000, 96_000),
        (96_000, 48_000),
        (44_100, 48_000),
        (48_000, 44_100),
    ] {
        for length in [16, 64, 512] {
            let positions = [0, 2, length - 3];
            let mut sofa = impulses(source_rate, length, &positions);
            resample_sofa(&mut sofa, target_rate).unwrap();
            let expected_length =
                (length as f64 * f64::from(target_rate) / f64::from(source_rate)).ceil() as usize;
            assert_eq!(sofa.ir_length, expected_length);
            assert_eq!(sofa.impulse_responses.len(), 6 * expected_length);
            assert_eq!(sofa.sample_rate, f64::from(target_rate));
            assert_eq!(sofa.data_sample_rate, Some(f64::from(target_rate)));
            for (measurement, position) in positions.into_iter().enumerate() {
                let offset = measurement * 2 * expected_length;
                let left = &sofa.impulse_responses[offset..offset + expected_length];
                let right =
                    &sofa.impulse_responses[offset + expected_length..offset + 2 * expected_length];
                let (peak, amplitude) = left
                    .iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
                    .unwrap();
                let expected_time =
                    position as f64 * f64::from(target_rate) / f64::from(source_rate);
                assert!(
                    amplitude.abs() > 0.01,
                    "lost impulse {source_rate}->{target_rate}, N={length}, n={position}, peak={amplitude}"
                );
                // A fractional physical time lies between adjacent output samples.
                assert!(
                    (peak as f64 - expected_time).abs() <= 1.0,
                    "extra resampler delay {source_rate}->{target_rate}, N={length}, n={position}, actual={peak}, expected={expected_time}"
                );
                for (&a, &b) in left.iter().zip(right) {
                    assert!(a.is_finite() && b.is_finite());
                    assert!((b + 0.5 * a).abs() < 2.0e-6);
                }
            }
        }
    }
}

#[test]
fn independent_passband_phase_has_no_backend_delay_or_fractional_offset() {
    let length = 2048;
    let position = length / 2;
    for (source_rate, target_rate) in [
        (48_000, 96_000),
        (96_000, 48_000),
        (44_100, 48_000),
        (48_000, 44_100),
    ] {
        let mut sofa = impulses(source_rate, length, &[position]);
        resample_sofa(&mut sofa, target_rate).unwrap();
        let left = &sofa.impulse_responses[..sofa.ir_length];
        for frequency in [100.0, 1000.0, 4000.0] {
            let (mut real, mut imaginary) = (0.0, 0.0);
            for (sample, &value) in left.iter().enumerate() {
                let phase =
                    -std::f64::consts::TAU * frequency * sample as f64 / f64::from(target_rate);
                real += f64::from(value) * phase.cos();
                imaginary += f64::from(value) * phase.sin();
            }
            // Preserve the existing sample-amplitude resampling convention.
            // Sampling the continuous impulse kernel scales its discrete sum by
            // the rate ratio. This test introduces no HRIR gain normalization.
            let magnitude = f64::from(target_rate) / f64::from(source_rate);
            let phase =
                -std::f64::consts::TAU * frequency * position as f64 / f64::from(source_rate);
            let error = (real - magnitude * phase.cos()).hypot(imaginary - magnitude * phase.sin())
                / magnitude;
            assert!(
                error < 2.0e-4,
                "passband phase/amplitude error {error} at {frequency} Hz, {source_rate}->{target_rate}"
            );
        }
    }
}

#[test]
fn same_rate_is_exact_and_measurements_have_independent_resampler_history() {
    let mut unchanged = impulses(48_000, 64, &[0, 2, 61]);
    let original = unchanged.impulse_responses.clone();
    resample_sofa(&mut unchanged, 48_000).unwrap();
    assert_eq!(unchanged.impulse_responses, original);
    assert_eq!(unchanged.ir_length, 64);

    let mut forward = impulses(44_100, 64, &[0, 2, 61]);
    let mut reverse = impulses(44_100, 64, &[61, 2, 0]);
    resample_sofa(&mut forward, 48_000).unwrap();
    resample_sofa(&mut reverse, 48_000).unwrap();
    let pair = forward.ir_length * 2;
    for measurement in 0..3 {
        assert_eq!(
            forward.impulse_responses[measurement * pair..(measurement + 1) * pair],
            reverse.impulse_responses[(2 - measurement) * pair..(3 - measurement) * pair]
        );
    }
}

#[test]
fn fractional_target_keeps_exact_clock_duration_and_independent_ears() {
    let source_rate = 48_000.0;
    let target_rate = 12_345.678;
    let length = 4_097;
    let mut sofa = impulses(48_000, length, &[0, length - 2]);
    resample_sofa(&mut sofa, target_rate).unwrap();
    let expected_length = (length as f64 * target_rate / source_rate).ceil() as usize;
    assert_eq!(sofa.sample_rate, target_rate);
    assert_eq!(sofa.data_sample_rate, Some(target_rate));
    assert_eq!(sofa.ir_length, expected_length);
    assert_eq!(sofa.impulse_responses.len(), 4 * expected_length);
    for (measurement, position) in [0, length - 2].into_iter().enumerate() {
        let offset = measurement * 2 * expected_length;
        let left = &sofa.impulse_responses[offset..offset + expected_length];
        let right = &sofa.impulse_responses[offset + expected_length..offset + 2 * expected_length];
        let (peak, amplitude) = left
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
            .unwrap();
        let expected_time = position as f64 * target_rate / source_rate;
        assert!(amplitude.abs() > 0.01);
        assert!((peak as f64 - expected_time).abs() <= 1.0);
        for (&left_sample, &right_sample) in left.iter().zip(right) {
            assert!((right_sample + 0.5 * left_sample).abs() < 2.0e-6);
        }
    }
}

#[test]
fn invalid_rates_dimensions_and_unsupported_grids_leave_source_unchanged() {
    for case in 0..10 {
        let mut sofa = impulses(48_000, 16, &[2]);
        let target = match case {
            0 => 0,
            1 => {
                sofa.sample_rate = 0.0;
                96_000
            }
            2 => {
                sofa.sample_rate = f64::NAN;
                96_000
            }
            3 => {
                sofa.sample_rate = f64::INFINITY;
                96_000
            }
            4 => {
                sofa.sample_rate = -48_000.0;
                96_000
            }
            5 => {
                sofa.impulse_responses.pop();
                96_000
            }
            6 => {
                sofa.num_measurements = usize::MAX;
                96_000
            }
            7 => {
                sofa.ir_length = usize::MAX;
                96_000
            }
            // Coprime rates must fail before allocating a huge backend, even
            // though this short IR's final output length is small.
            8 => {
                sofa.sample_rate = 1_000_003.0;
                1_000_033
            }
            9 => {
                sofa.sample_rate = f64::from(u32::MAX) + 1.0;
                96_000
            }
            _ => unreachable!(),
        };
        let samples = sofa.impulse_responses.clone();
        let length = sofa.ir_length;
        let rate = sofa.sample_rate.to_bits();
        let data_rate = sofa.data_sample_rate;
        let measurements = sofa.num_measurements;
        let error = resample_sofa(&mut sofa, target).unwrap_err();
        if case == 8 {
            assert!(error.contains("Unsupported HRTF resampling FFT grid"));
        }
        assert_eq!(sofa.impulse_responses, samples, "case {case}");
        assert_eq!(sofa.ir_length, length, "case {case}");
        assert_eq!(sofa.sample_rate.to_bits(), rate, "case {case}");
        assert_eq!(sofa.data_sample_rate, data_rate, "case {case}");
        assert_eq!(sofa.num_measurements, measurements, "case {case}");
    }
}

#[test]
fn empty_datasets_convert_metadata_without_a_backend() {
    for (measurements, length) in [(0, 0), (0, 16), (2, 0)] {
        let mut sofa = impulses(48_000, 0, &[]);
        sofa.num_measurements = measurements;
        sofa.ir_length = length;
        sofa.positions = vec![SourcePosition::new(0.0, 0.0, 1.0); measurements];
        resample_sofa(&mut sofa, 96_000).unwrap();
        assert!(sofa.impulse_responses.is_empty());
        assert_eq!(sofa.num_measurements, measurements);
        assert_eq!(sofa.ir_length, length * 2);
        assert_eq!(sofa.sample_rate, 96_000.0);
        assert_eq!(sofa.data_sample_rate, Some(96_000.0));
    }
}
