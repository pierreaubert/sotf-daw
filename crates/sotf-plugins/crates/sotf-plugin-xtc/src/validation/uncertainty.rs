//! Supplied transfer-error envelopes for prepared or rendered XTC matrices.

// Rust guideline compliant 2026-02-21
use super::matrix::ComplexTransferMatrix;
use rustfft::num_complex::Complex;

/// Absolute ear-response intervals for each intended input cue.
///
/// Each row is an intended left/right input, with `[lower, upper]` endpoints.
/// Amplitudes use the supplied coherent transfer reference. Separation is the
/// desired/undesired ear amplitude ratio in dB, not improvement over passthrough.
/// A muted input has undefined (`NaN`) separation, rather than perfect isolation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CancellationEnvelope {
    /// Bounds on intended-ear amplitude for left/right input cues.
    pub desired_amplitude: [[f64; 2]; 2],
    /// Bounds on unintended-ear amplitude for left/right input cues.
    pub undesired_amplitude: [[f64; 2]; 2],
    /// Bounds on absolute intended/unintended separation for each input, in dB.
    pub separation_db: [[f64; 2]; 2],
}

/// Bounds cancellation for explicit per-ear transfer-row error radii.
///
/// For each ear `i`, the caller asserts `||H_true[i,:] - H[i,:]||_2 <= error[i]`.
/// For a filter column `w`, Cauchy–Schwarz bounds response error by
/// `error[i] * ||w||_2`. The triangle inequality then gives amplitude and
/// separation intervals. The function uses f64 arithmetic and allocates nothing.
///
/// Bounds must come from a stated measurement/error model; a singular value or
/// listening radius alone is not a transfer-error bound. This function does not
/// infer provenance or certify a human HRTF. Comparing two lower separation
/// bounds does not prove absence of degradation. A filtered lower bound above
/// the passthrough upper bound is a conservative sufficient comparison.
///
/// Returns `None` for negative/non-finite error bounds, non-finite matrix entries
/// or arithmetic overflow. Zero residual can produce infinite separation; zero
/// desired and undesired amplitudes produce `NaN`, preserving muted-cue ambiguity.
/// The reported intervals bound the stated linear matrices, not STFT transients,
/// clipping, AutoGain, nonlinear processing or physical device behavior.
///
/// # Examples
/// ```
/// use sotf_plugin_xtc::validation::bound_cancellation;
/// let identity = [[[1.0, 0.0], [0.0, 0.0]], [[0.0, 0.0], [1.0, 0.0]]];
/// let bounds = bound_cancellation(identity, identity, [0.1; 2]).unwrap();
/// assert_eq!(bounds.desired_amplitude[0], [0.9, 1.1]);
/// assert!(bounds.separation_db[0][0] > 19.0);
/// ```
pub fn bound_cancellation(
    plant: ComplexTransferMatrix,
    filter: ComplexTransferMatrix,
    row_error: [f64; 2],
) -> Option<CancellationEnvelope> {
    if row_error.iter().any(|v| !v.is_finite() || *v < 0.0)
        || plant
            .iter()
            .chain(&filter)
            .flatten()
            .flatten()
            .any(|v| !v.is_finite())
    {
        return None;
    }
    let unpack = |m: ComplexTransferMatrix| m.map(|row| row.map(|[re, im]| Complex::new(re, im)));
    let plant = unpack(plant);
    let filter = unpack(filter);
    let amplitude = |value: Complex<f64>| value.re.hypot(value.im);
    let mut desired_amplitude = [[0.0; 2]; 2];
    let mut undesired_amplitude = [[0.0; 2]; 2];
    let mut separation_db = [[0.0; 2]; 2];
    for input in 0..2 {
        let column_norm = amplitude(filter[0][input]).hypot(amplitude(filter[1][input]));
        let mut intervals = [[0.0; 2]; 2];
        for ear in 0..2 {
            let response = plant[ear][0] * filter[0][input] + plant[ear][1] * filter[1][input];
            let nominal = amplitude(response);
            let error = row_error[ear] * column_norm;
            let upper = nominal + error;
            if !upper.is_finite() || !error.is_finite() {
                return None;
            }
            intervals[ear] = [(nominal - error).max(0.0), upper];
        }
        let desired = intervals[input];
        let undesired = intervals[1 - input];
        desired_amplitude[input] = desired;
        undesired_amplitude[input] = undesired;
        separation_db[input] = [
            20.0 * (desired[0].log10() - undesired[1].log10()),
            20.0 * (desired[1].log10() - undesired[0].log10()),
        ];
    }
    Some(CancellationEnvelope {
        desired_amplitude,
        undesired_amplitude,
        separation_db,
    })
}

#[cfg(test)]
#[path = "uncertainty_tests.rs"]
mod tests;
