//! Sampled final-coefficient qualification for opt-in automatic FIR support.

// Rust guideline compliant 2026-02-21
use super::eq_band::EqBand;
use super::types::LinearPhaseEqBandPlacement as Placement;
use math_audio_iir_fir::BiquadFilterType;
use num_complex::Complex;
use realfft::RealFftPlanner;

/// Maximum sampled magnitude-deviation bound requested by accuracy mode.
pub const ACCURACY_TOLERANCE_DB: f64 = 0.05;
/// Absolute reference floor for the sampled magnitude-residual criterion.
///
/// Below -80 dB, qualification bounds absolute magnitude residual by
/// `(1 - 10^(-0.05/20)) * 1e-4`, approximately 5.74e-7 amplitude.
pub const ACCURACY_REFERENCE_FLOOR: f64 = 1e-4;

/// Resolved FIR support and sampled response qualification before processing.
///
/// This reports the complete wet route, including each ordered stage's
/// partition latency. The finite grid is not a continuous-frequency proof.
/// Scalar routes qualify magnitude residual. Ordered paired routes additionally
/// qualify complex operator error relative to the target's weakest direction
/// (with the explicit reference floor near nulls). Minimum-phase target stages
/// retain each constructed FIR's unit phase and replace only its magnitude by
/// the independent ideal magnitude. This qualifies the cascade given actual
/// stage phase; existing phase-design tests remain separate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FirAccuracyReport {
    /// Actual selected support per stage, independent of the saved manual choice.
    pub selected_taps: usize,
    /// Number of convolution stages in the complete route.
    pub stage_count: usize,
    /// Full reported algorithmic latency in input samples.
    pub latency_samples: usize,
    /// Complete route latency for the unchanged saved manual FIR choice.
    pub manual_latency_samples: usize,
    /// Extra samples over the manual choice, zero when selection is shorter.
    pub additional_latency_samples: usize,
    /// Prepared sample rate used by this qualification.
    pub sample_rate: f64,
    /// Largest conservative sampled magnitude-deviation bound in dB.
    pub maximum_error_db: f64,
    /// Frequency of the largest sampled deviation.
    pub worst_frequency_hz: f64,
    /// Number of dense and feature-aware frequency checks.
    pub frequency_points: usize,
    /// Whether the ordered paired route also qualified its complex operator error.
    pub paired_operator_qualified: bool,
}

#[derive(Clone, Copy, Debug)]
pub(super) struct Measurement {
    pub(super) maximum_error_db: f64,
    pub(super) worst_frequency_hz: f64,
    pub(super) frequency_points: usize,
    pub(super) paired_operator_qualified: bool,
}

type C = Complex<f64>;
type Matrix = [[C; 2]; 2];
const ONE: C = C::new(1.0, 0.0);
const ZERO: C = C::new(0.0, 0.0);

/// Independent bilinear analog prototype, without production biquad coefficients.
fn prototype(band: &EqBand, frequency: f64, rate: f64) -> C {
    if !band.active {
        return ONE;
    }
    let a = 10.0_f64.powf(band.gain_db / 40.0);
    if frequency >= rate * 0.5 {
        return match band.filter_type {
            BiquadFilterType::Lowpass => ZERO,
            BiquadFilterType::Highshelf => C::new(a * a, 0.0),
            _ => ONE,
        };
    }
    let ratio = (std::f64::consts::PI * frequency / rate).tan()
        / (std::f64::consts::PI * band.frequency / rate).tan();
    let square = -ratio * ratio;
    let p = C::new(0.0, ratio);
    match band.filter_type {
        BiquadFilterType::Peak => {
            (C::new(1.0 + square, 0.0) + p * (a / band.q))
                / (C::new(1.0 + square, 0.0) + p / (a * band.q))
        }
        BiquadFilterType::Lowpass => ONE / (C::new(1.0 + square, 0.0) + p / band.q),
        BiquadFilterType::Highpass => {
            C::new(square, 0.0) / (C::new(1.0 + square, 0.0) + p / band.q)
        }
        BiquadFilterType::Lowshelf => {
            a * (C::new(a + square, 0.0) + p * (2.0 * a).sqrt())
                / (C::new(1.0 + a * square, 0.0) + p * (2.0 * a).sqrt())
        }
        BiquadFilterType::Highshelf => {
            a * (C::new(1.0 + a * square, 0.0) + p * (2.0 * a).sqrt())
                / (C::new(a + square, 0.0) + p * (2.0 * a).sqrt())
        }
        _ => ONE,
    }
}

fn stage_matrix(value: C, placement: Placement) -> Matrix {
    match placement {
        Placement::Stereo => [[value, ZERO], [ZERO, value]],
        Placement::Left => [[value, ZERO], [ZERO, ONE]],
        Placement::Right => [[ONE, ZERO], [ZERO, value]],
        Placement::Mid => {
            let diagonal = (ONE + value) * 0.5;
            let cross = (value - ONE) * 0.5;
            [[diagonal, cross], [cross, diagonal]]
        }
        Placement::Side => {
            let diagonal = (ONE + value) * 0.5;
            let cross = (ONE - value) * 0.5;
            [[diagonal, cross], [cross, diagonal]]
        }
    }
}

fn multiply(a: Matrix, b: Matrix) -> Matrix {
    std::array::from_fn(|i| std::array::from_fn(|j| a[i][0] * b[0][j] + a[i][1] * b[1][j]))
}

fn singular_values(matrix: Matrix) -> (f64, f64) {
    let power = matrix.iter().flatten().map(Complex::norm_sqr).sum::<f64>();
    let determinant = (matrix[0][0] * matrix[1][1] - matrix[0][1] * matrix[1][0]).norm();
    let maximum = ((power
        + (power * power - 4.0 * determinant * determinant)
            .max(0.0)
            .sqrt())
        * 0.5)
        .sqrt();
    let minimum = if maximum > 0.0 {
        determinant / maximum
    } else {
        0.0
    };
    (minimum, maximum)
}

fn operator_upper_bound(matrix: Matrix) -> f64 {
    let frobenius = matrix
        .iter()
        .flatten()
        .map(Complex::norm_sqr)
        .sum::<f64>()
        .sqrt();
    // The discriminant can lose relative precision at repeated singular
    // values. Cover its square-root amplification instead of using a tiny
    // relative padding on the computed singular value alone.
    singular_values(matrix).1 + 8.0 * f64::EPSILON.sqrt() * frobenius
}

fn minimum_singular_lower_bound(matrix: Matrix) -> f64 {
    let first = matrix[0][0] * matrix[1][1];
    let second = matrix[0][1] * matrix[1][0];
    let determinant = (first - second).norm();
    // Cancellation in the complex determinant must not overstate the weak
    // direction of a highly conditioned cascade. An unresolved determinant
    // lowers the reference to zero; the declared absolute floor then applies.
    let allowance = 32.0 * f64::EPSILON * (first.norm() + second.norm());
    let maximum = operator_upper_bound(matrix);
    if maximum > 0.0 {
        (determinant - allowance).max(0.0) / maximum
    } else {
        0.0
    }
}

fn deviation_bound_db(residual: f64, reference: f64) -> f64 {
    let relative = residual / reference.max(ACCURACY_REFERENCE_FLOOR);
    if relative.is_finite() && relative < 1.0 {
        -20.0 * (-relative).ln_1p() / std::f64::consts::LN_10
    } else {
        f64::INFINITY
    }
}

fn dtft(taps: &[f32], frequency: f64, rate: f64) -> C {
    let step = C::from_polar(1.0, -std::f64::consts::TAU * frequency / rate);
    let mut phase = ONE;
    let mut response = ZERO;
    for &tap in taps {
        response += phase * f64::from(tap);
        phase *= step;
    }
    response
}

fn feature_frequencies(bands: &[EqBand], rate: f64) -> Vec<f64> {
    let nyquist = rate * 0.5;
    let mut points = vec![0.0, nyquist];
    // An independent logarithmic grid supplements uniformly spaced FFT bins.
    for i in 0..=256 {
        points.push((nyquist.ln() * i as f64 / 256.0).exp().min(nyquist));
    }
    for band in bands.iter().filter(|band| band.active) {
        // Resolve centers and Q-dependent transition neighborhoods without
        // supplying any audit/test probe frequencies to the selector.
        for offset in -64..=64 {
            points.push(
                (band.frequency * 2.0_f64.powf(f64::from(offset) / (16.0 * band.q)))
                    .clamp(0.0, nyquist),
            );
        }
    }
    points.sort_by(f64::total_cmp);
    points.dedup();
    points
}

/// Qualify final f32 stage coefficients and their complete channel-domain route.
#[allow(
    clippy::too_many_arguments,
    reason = "explicit immutable prepared route description"
)]
pub(super) fn measure(
    firs: &[Vec<f32>],
    bands: &[EqBand],
    ordered: bool,
    phase_mode: usize,
    auto_gain: bool,
    channels: usize,
    pairs: &[[usize; 2]],
    rate: f64,
) -> Result<Measurement, String> {
    let taps = firs.first().map_or(0, Vec::len);
    if taps == 0 || channels == 0 || firs.iter().any(|fir| fir.len() != taps) {
        return Err("accuracy mode requires nonempty equal-length FIR stages and channels".into());
    }
    let size = (32 * taps).next_power_of_two().max(32768);
    let mut planner = RealFftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(size);
    let mut spectra = Vec::with_capacity(firs.len());
    for fir in firs {
        let mut input = forward.make_input_vec();
        for (dst, &src) in input.iter_mut().zip(fir) {
            *dst = f64::from(src);
        }
        let mut output = forward.make_output_vec();
        forward
            .process(&mut input, &mut output)
            .map_err(|error| format!("accuracy qualification FFT failed: {error}"))?;
        spectra.push(output);
    }
    let mut target_scale = 1.0;
    if auto_gain {
        let combined = |frequency| {
            bands
                .iter()
                .map(|band| prototype(band, frequency, rate))
                .product::<C>()
                .norm()
        };
        let dc = combined(0.0);
        let nyquist = combined(rate * 0.5);
        let reference = if dc > 1e-4 {
            dc
        } else if nyquist > 1e-4 {
            nyquist
        } else {
            1.0
        };
        target_scale = 1.0 / reference;
    }
    let features = feature_frequencies(bands, rate);
    let mut result = Measurement {
        maximum_error_db: 0.0,
        worst_frequency_hz: 0.0,
        frequency_points: size / 2 + 1 + features.len(),
        paired_operator_qualified: ordered && !pairs.is_empty(),
    };
    let mut compare = |residual: f64, reference: f64, frequency: f64| {
        let error = deviation_bound_db(residual, reference);
        if error > result.maximum_error_db {
            result.maximum_error_db = error;
            result.worst_frequency_hz = frequency;
        }
    };
    let mut actual = vec![ZERO; firs.len()];
    for (index, frequency) in (0..=size / 2)
        .map(|i| (Some(i), i as f64 * rate / size as f64))
        .chain(features.iter().copied().map(|frequency| (None, frequency)))
    {
        let dephase = if phase_mode == 0 {
            C::from_polar(
                1.0,
                std::f64::consts::TAU * frequency * (taps / 2) as f64 / rate,
            )
        } else {
            ONE
        };
        for (stage, fir) in firs.iter().enumerate() {
            actual[stage] = index
                .map_or_else(|| dtft(fir, frequency, rate), |bin| spectra[stage][bin])
                * dephase;
        }
        if !ordered {
            let target = bands
                .iter()
                .map(|band| prototype(band, frequency, rate))
                .product::<C>();
            let target = (target * target_scale).norm();
            compare((actual[0].norm() - target).abs(), target, frequency);
            continue;
        }
        let mut actual_pair = [[ONE, ZERO], [ZERO, ONE]];
        let mut target_pair = actual_pair;
        let mut actual_unpaired = ONE;
        let mut target_unpaired = ONE;
        for (stage, band) in bands.iter().enumerate() {
            let raw_target = prototype(band, frequency, rate);
            // Minimum-phase qualification follows actual stage phase. This
            // checks magnitudes/cascade, not an independent phase oracle.
            let target = if phase_mode == 0 || actual[stage].norm() == 0.0 {
                C::new(raw_target.norm(), 0.0)
            } else {
                actual[stage] / actual[stage].norm() * raw_target.norm()
            };
            compare(
                (actual[stage].norm() - target.norm()).abs(),
                target.norm(),
                frequency,
            );
            let placement = band.placement.unwrap_or(Placement::Stereo);
            actual_pair = multiply(stage_matrix(actual[stage], placement), actual_pair);
            target_pair = multiply(stage_matrix(target, placement), target_pair);
            if placement == Placement::Stereo {
                actual_unpaired *= actual[stage];
                target_unpaired *= target;
            }
        }
        if !pairs.is_empty() {
            let difference = std::array::from_fn(|row| {
                std::array::from_fn(|col| actual_pair[row][col] - target_pair[row][col])
            });
            compare(
                operator_upper_bound(difference),
                minimum_singular_lower_bound(target_pair),
                frequency,
            );
        }
        if channels > 2 * pairs.len() {
            compare(
                (actual_unpaired - target_unpaired).norm(),
                target_unpaired.norm(),
                frequency,
            );
        }
    }
    Ok(result)
}

#[cfg(test)]
#[path = "accuracy_tests.rs"]
mod tests;
