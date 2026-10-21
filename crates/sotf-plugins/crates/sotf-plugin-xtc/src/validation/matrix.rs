//! Signed cancellation metrics for externally supplied complex transfer matrices.

// Rust guideline compliant 2026-02-21
use rustfft::num_complex::Complex;

/// A two-by-two complex matrix with `[real, imaginary]` entries.
///
/// For a plant, rows are ears and columns are speakers. For a filter, rows are
/// speaker outputs and columns are intended-ear inputs. All entries must refer
/// to the same physical frequency and coherent timing reference.
pub type ComplexTransferMatrix = [[[f64; 2]; 2]; 2];

/// Signed crosstalk and intended-signal changes for left and right input cues.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CancellationMetrics {
    /// Raw undesired-ear attenuation, in dB; negative values mean degradation.
    pub raw_depth_db: [f64; 2],
    /// Intended-ear gain relative to the unfiltered plant, in dB.
    pub desired_gain_db: [f64; 2],
    /// Crosstalk reduction relative to intended-ear level, in dB.
    ///
    /// This equals raw depth plus desired gain. It does not credit turning down
    /// both ears equally. A muted cue has an undefined (`NaN`) separation change.
    pub separation_improvement_db: [f64; 2],
}

/// Evaluate signed cancellation using supplied plant and filter transfer matrices.
///
/// Computes `T = H * W` in f64. Unlike a model prediction, `H` may contain
/// independently measured speaker-to-ear responses and `W` may come from actual
/// rendered speaker outputs. No inverse-design or head-model helper is called.
/// Matrix entries alone do not establish their measurement provenance.
///
/// Results are ordered by intended left/right input, not by the receiving ear.
/// Values are uncapped: perfect zero residual can produce positive infinity,
/// newly introduced crosstalk can produce negative infinity, and a zero-over-zero
/// comparison is undefined (`NaN`). Non-finite inputs or overflow produce `NaN`
/// metrics. Intended gain and normalized separation must accompany raw depth;
/// attenuation of both ears or muting is not evidence of useful cancellation.
///
/// # Examples
/// ```
/// use sotf_plugin_xtc::validation::evaluate_cancellation;
/// let plant = [[[1.0, 0.0], [0.2, 0.0]], [[0.2, 0.0], [1.0, 0.0]]];
/// let identity = [[[1.0, 0.0], [0.0, 0.0]], [[0.0, 0.0], [1.0, 0.0]]];
/// let metrics = evaluate_cancellation(plant, identity);
/// assert_eq!(metrics.raw_depth_db, [0.0; 2]);
/// assert_eq!(metrics.separation_improvement_db, [0.0; 2]);
/// ```
pub fn evaluate_cancellation(
    plant: ComplexTransferMatrix,
    filter: ComplexTransferMatrix,
) -> CancellationMetrics {
    let invalid = CancellationMetrics {
        raw_depth_db: [f64::NAN; 2],
        desired_gain_db: [f64::NAN; 2],
        separation_improvement_db: [f64::NAN; 2],
    };
    if plant
        .iter()
        .chain(&filter)
        .flatten()
        .flatten()
        .any(|v| !v.is_finite())
    {
        return invalid;
    }
    let h: [[Complex<f64>; 2]; 2] = plant.map(|row| row.map(|[re, im]| Complex::new(re, im)));
    let w: [[Complex<f64>; 2]; 2] = filter.map(|row| row.map(|[re, im]| Complex::new(re, im)));
    let t: [[Complex<f64>; 2]; 2] = std::array::from_fn(|ear| {
        std::array::from_fn(|input| h[ear][0] * w[0][input] + h[ear][1] * w[1][input])
    });
    if t.iter()
        .flatten()
        .any(|c| !c.re.is_finite() || !c.im.is_finite())
    {
        return invalid;
    }
    let amplitude = |c: Complex<f64>| c.re.hypot(c.im);
    let db_ratio = |a: f64, b: f64| 20.0 * (a.log10() - b.log10());
    let raw_depth_db = std::array::from_fn(|input| {
        db_ratio(
            amplitude(h[1 - input][input]),
            amplitude(t[1 - input][input]),
        )
    });
    let desired_gain_db = std::array::from_fn(|input| {
        db_ratio(amplitude(t[input][input]), amplitude(h[input][input]))
    });
    let separation_improvement_db =
        std::array::from_fn(|input| raw_depth_db[input] + desired_gain_db[input]);
    CancellationMetrics {
        raw_depth_db,
        desired_gain_db,
        separation_improvement_db,
    }
}
