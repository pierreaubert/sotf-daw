//! Editable frequency-dependent reduction curve.
//!
//! [`ReductionCurve`] scales the denoiser's computed per-bin reduction with
//! three knots at fixed physical frequencies. Knots persist as the
//! `curve_low`, `curve_mid`, and `curve_high` parameters; bin frequencies in
//! Hz select the interpolated scale, so the curve survives FFT-size and
//! sample-rate changes by construction.
//!
//! Interpolation mirrors the Hiss reducer restoration-family contract: linear
//! in log frequency between anchors, clamped outside them, evaluated in f64.
//! Anchor positions differ because the denoiser covers the full band while
//! Hiss focuses on high-band content; see [`DENOISER_CURVE_ANCHOR_HZ`].

/// Fixed log-spaced reduction-curve anchors in Hz (low, mid, high).
///
/// ISO octave-band centers three octaves apart: 125 Hz (lows and rumble),
/// 1000 Hz (speech presence), and 8000 Hz (brilliance and hiss). Content at
/// or below 125 Hz uses the low knot; content at or above 8000 Hz uses the
/// high knot. These coordinates are part of the persisted interpolation
/// contract: changing them alters the sound of stored presets, so any future
/// revision needs a migration version, never a silent edit.
pub const DENOISER_CURVE_ANCHOR_HZ: [f32; 3] = [125.0, 1000.0, 8000.0];

/// Three-knot frequency-dependent reduction scale.
///
/// Each knot holds the fraction of computed reduction retained at its
/// anchor: 1.0 keeps full reduction, 0.0 disables reduction (unity gain).
/// Knots are canonicalized to 0.0..=1.0; see [`ReductionCurve::canonicalize`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ReductionCurve {
    /// Reduction scale at or below 125 Hz.
    pub low: f32,
    /// Reduction scale at 1000 Hz.
    pub mid: f32,
    /// Reduction scale at or above 8000 Hz.
    pub high: f32,
}

impl Default for ReductionCurve {
    fn default() -> Self {
        Self {
            low: 1.0,
            mid: 1.0,
            high: 1.0,
        }
    }
}

impl ReductionCurve {
    /// Clamps one curve control point, repairing non-finite input to 1.0.
    ///
    /// # Examples
    ///
    /// ```
    /// use sotf_plugin_denoiser::ReductionCurve;
    ///
    /// assert_eq!(ReductionCurve::canonicalize(0.3), 0.3);
    /// assert_eq!(ReductionCurve::canonicalize(-0.5), 0.0);
    /// assert_eq!(ReductionCurve::canonicalize(1.5), 1.0);
    /// assert_eq!(ReductionCurve::canonicalize(f32::NAN), 1.0);
    /// ```
    pub fn canonicalize(value: f32) -> f32 {
        if value.is_finite() {
            value.clamp(0.0, 1.0)
        } else {
            1.0
        }
    }

    /// Returns true when the curve keeps full reduction everywhere.
    ///
    /// The DSP path skips per-bin scaling for flat curves, which preserves
    /// legacy bit-identical output when the curve was never edited.
    pub fn is_flat(&self) -> bool {
        self.low == 1.0 && self.mid == 1.0 && self.high == 1.0
    }

    /// Interpolates the reduction scale at `freq_hz`.
    ///
    /// Interpolation is linear in log frequency between the fixed anchors
    /// and clamps outside them, so anchor values reproduce exactly.
    /// Non-finite or non-positive frequencies return the low knot.
    ///
    /// # Examples
    ///
    /// ```
    /// use sotf_plugin_denoiser::{DENOISER_CURVE_ANCHOR_HZ, ReductionCurve};
    ///
    /// let curve = ReductionCurve {
    ///     low: 0.0,
    ///     mid: 0.5,
    ///     high: 1.0,
    /// };
    /// assert_eq!(curve.gain_at(DENOISER_CURVE_ANCHOR_HZ[0]), 0.0);
    /// assert_eq!(curve.gain_at(DENOISER_CURVE_ANCHOR_HZ[1]), 0.5);
    /// assert_eq!(curve.gain_at(DENOISER_CURVE_ANCHOR_HZ[2]), 1.0);
    /// assert_eq!(curve.gain_at(20.0), 0.0);
    /// assert_eq!(curve.gain_at(20_000.0), 1.0);
    /// ```
    pub fn gain_at(&self, freq_hz: f32) -> f32 {
        if !freq_hz.is_finite() || freq_hz <= DENOISER_CURVE_ANCHOR_HZ[0] {
            return self.low;
        }
        if freq_hz >= DENOISER_CURVE_ANCHOR_HZ[2] {
            return self.high;
        }
        let (lo_f, hi_f, lo_g, hi_g) = if freq_hz < DENOISER_CURVE_ANCHOR_HZ[1] {
            (
                DENOISER_CURVE_ANCHOR_HZ[0],
                DENOISER_CURVE_ANCHOR_HZ[1],
                self.low,
                self.mid,
            )
        } else {
            (
                DENOISER_CURVE_ANCHOR_HZ[1],
                DENOISER_CURVE_ANCHOR_HZ[2],
                self.mid,
                self.high,
            )
        };
        let position =
            (f64::from(freq_hz) / f64::from(lo_f)).ln() / (f64::from(hi_f) / f64::from(lo_f)).ln();
        (f64::from(lo_g) + position * (f64::from(hi_g) - f64::from(lo_g))) as f32
    }

    /// Shapes one linear Wiener-style gain with a curve scale.
    ///
    /// `scale` is the interpolated 0.0..=1.0 reduction fraction and `gain`
    /// the computed bin gain in `floor_linear..=1.0`. The result stays in
    /// `gain..=1.0`, so the gain floor still bounds the output and a scale
    /// of 1.0 requests the unshaped gain.
    pub fn shape_gain(scale: f32, gain: f32) -> f32 {
        1.0 - scale * (1.0 - gain)
    }
}
