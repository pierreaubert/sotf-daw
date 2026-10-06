use super::misc::bandpass_edges;
use crate::params::{DynEqPlacement, DynEqShape, default_band_ratio, default_band_threshold};
use math_audio_iir_fir::{Biquad, BiquadCoefficients, BiquadFilterType};
use sotf_host::dynamics_core::DynamicsCore;
use sotf_host::dynamics_core::DynamicsMode;

pub(super) struct DynEqBand {
    // EQ parameters
    pub(super) frequency: f32,
    pub(super) q: f32,
    pub(super) target_gain_db: f32,
    pub(super) shape: DynEqShape,
    pub(super) shelf_slope: f32,
    // Stereo-field routing; `Stereo` keeps the exact legacy path.
    pub(super) placement: DynEqPlacement,

    // Per-band dynamics overrides
    pub(super) band_threshold: f32,
    pub(super) band_ratio: f32,
    pub(super) use_band_threshold: bool,
    pub(super) use_band_ratio: bool,

    // Band control
    pub(super) active: bool,
    pub(super) solo: bool,

    // DSP state (pre-allocated for max channels)
    /// Highpass filter per channel (lower bound of sidechain BPF)
    pub(super) sidechain_bp_hp: Vec<Biquad>,
    /// Lowpass filter per channel (upper bound of sidechain BPF)
    pub(super) sidechain_bp_lp: Vec<Biquad>,
    /// The actual EQ biquad per channel — held at target_gain_db (static coefficients).
    /// Gain modulation is applied as a dry/wet blend, not via coefficient updates.
    pub(super) eq_filters: Vec<Biquad>,
    /// Prepared full-target non-peak coefficients (shelves and tilt). Peak
    /// bands keep the original coefficient-owning `Biquad::process` path.
    non_peak_coefficients: Option<BiquadCoefficients<f64>>,
    /// One DynamicsCore per channel
    pub(super) cores: Vec<DynamicsCore>,
}

impl DynEqBand {
    pub(super) fn new(
        channels: usize,
        sample_rate: impl Into<f64>,
        frequency: f32,
        q: f32,
        target_gain_db: f32,
        attack_ms: f32,
        release_ms: f32,
    ) -> Self {
        let sample_rate = sample_rate.into();
        let (f_low, f_high) = bandpass_edges(frequency, q);

        let sidechain_bp_hp = (0..channels)
            .map(|_| {
                Biquad::new(
                    BiquadFilterType::Highpass,
                    f_low as f64,
                    sample_rate,
                    std::f64::consts::FRAC_1_SQRT_2,
                    0.0,
                )
            })
            .collect();

        let sidechain_bp_lp = (0..channels)
            .map(|_| {
                Biquad::new(
                    BiquadFilterType::Lowpass,
                    f_high as f64,
                    sample_rate,
                    std::f64::consts::FRAC_1_SQRT_2,
                    0.0,
                )
            })
            .collect();

        let eq_filters = (0..channels)
            .map(|_| {
                Biquad::new(
                    BiquadFilterType::Peak,
                    frequency as f64,
                    sample_rate,
                    q as f64,
                    0.0, // starts at 0 dB (passthrough)
                )
            })
            .collect();

        let mut cores: Vec<DynamicsCore> = (0..channels)
            .map(|_| DynamicsCore::new(DynamicsMode::Compress, 1, sample_rate))
            .collect();
        for core in &mut cores {
            core.set_attack_release(attack_ms, release_ms);
        }

        Self {
            frequency,
            q,
            target_gain_db,
            shape: DynEqShape::Peak,
            shelf_slope: 1.0,
            placement: DynEqPlacement::Stereo,
            band_threshold: default_band_threshold(),
            band_ratio: default_band_ratio(),
            use_band_threshold: false,
            use_band_ratio: false,
            active: true,
            solo: false,
            sidechain_bp_hp,
            sidechain_bp_lp,
            eq_filters,
            non_peak_coefficients: None,
            cores,
        }
    }

    /// Process the sidechain bandpass filter on a sample for a given channel.
    ///
    /// Accepts and returns f64 to avoid unnecessary round-trip conversions; the
    /// internal biquads already operate in f64.
    #[inline]
    pub(super) fn apply_sidechain_bp(&mut self, ch: usize, sample: f64) -> f64 {
        match self.shape {
            DynEqShape::Peak => {
                let hp_out = self.sidechain_bp_hp[ch].process(sample);
                self.sidechain_bp_lp[ch].process(hp_out)
            }
            DynEqShape::LowShelf => self.sidechain_bp_lp[ch].process(sample),
            DynEqShape::HighShelf => self.sidechain_bp_hp[ch].process(sample),
            // Tilt uses a full-band detector: the pivot curve has no single
            // passband, so the unfiltered routed sample drives the envelope.
            DynEqShape::Tilt => sample,
        }
    }

    /// Compute the proportion of EQ gain to apply based on gain reduction from the
    /// dynamics core. Returns a value in [0.0, 1.0] representing how much of the
    /// full EQ band shape to blend in.
    #[inline]
    pub(super) fn modulation_proportion(target_gain_db: f32, gain_reduction_db: f32) -> f32 {
        if target_gain_db.abs() < 0.01 {
            return 0.0;
        }
        let applied_db =
            gain_reduction_db.clamp(0.0, target_gain_db.abs()) * target_gain_db.signum();
        let full_amplitude = 10.0f32.powf(target_gain_db / 20.0);
        let desired_amplitude = 10.0f32.powf(applied_db / 20.0);
        ((desired_amplitude - 1.0) / (full_amplitude - 1.0)).clamp(0.0, 1.0)
    }

    fn max_frequency(sample_rate: f64) -> f32 {
        (sample_rate as f32 * 0.475).min(20_000.0)
    }

    pub(super) fn preflight_reinitialize(&self, sample_rate: impl Into<f64>) -> Result<(), String> {
        let sample_rate = sample_rate.into();
        if self.shape == DynEqShape::Peak {
            return Ok(());
        }

        let maximum = Self::max_frequency(sample_rate);
        if !self.frequency.is_finite() || !(20.0..=maximum).contains(&self.frequency) {
            let kind = if self.shape == DynEqShape::Tilt {
                "tilt pivot"
            } else {
                "shelf cutoff"
            };
            return Err(format!(
                "{kind} {} Hz is outside 20..={maximum} Hz at {sample_rate} Hz",
                self.frequency
            ));
        }
        let valid = match self.shape {
            DynEqShape::Tilt => design_tilt_coefficients(
                self.frequency as f64,
                sample_rate,
                self.target_gain_db as f64,
            )
            .is_some(),
            _ => design_shelf_coefficients(
                self.shape,
                self.frequency as f64,
                sample_rate,
                self.target_gain_db as f64,
                self.shelf_slope as f64,
            )
            .is_some(),
        };
        if !valid {
            let kind = if self.shape == DynEqShape::Tilt {
                "tilt"
            } else {
                "shelf"
            };
            return Err(format!(
                "{kind} coefficients are invalid at {} Hz for {sample_rate} Hz",
                self.frequency
            ));
        }
        Ok(())
    }

    pub(super) fn rebuild_sidechain_filters(&mut self, sample_rate: impl Into<f64>) {
        let sample_rate = sample_rate.into();
        self.frequency = self.frequency.clamp(20.0, Self::max_frequency(sample_rate));
        match self.shape {
            DynEqShape::Peak => {
                let (f_low, f_high) = bandpass_edges(self.frequency, self.q);
                let f_high = f_high.min(Self::max_frequency(sample_rate));
                for hp in &mut self.sidechain_bp_hp {
                    *hp = Biquad::new(
                        BiquadFilterType::Highpass,
                        f_low as f64,
                        sample_rate,
                        std::f64::consts::FRAC_1_SQRT_2,
                        0.0,
                    );
                }
                for lp in &mut self.sidechain_bp_lp {
                    *lp = Biquad::new(
                        BiquadFilterType::Lowpass,
                        f_high as f64,
                        sample_rate,
                        std::f64::consts::FRAC_1_SQRT_2,
                        0.0,
                    );
                }
            }
            DynEqShape::LowShelf => {
                for lp in &mut self.sidechain_bp_lp {
                    *lp = Biquad::new(
                        BiquadFilterType::Lowpass,
                        self.frequency as f64,
                        sample_rate,
                        std::f64::consts::FRAC_1_SQRT_2,
                        0.0,
                    );
                }
            }
            DynEqShape::HighShelf => {
                for hp in &mut self.sidechain_bp_hp {
                    *hp = Biquad::new(
                        BiquadFilterType::Highpass,
                        self.frequency as f64,
                        sample_rate,
                        std::f64::consts::FRAC_1_SQRT_2,
                        0.0,
                    );
                }
            }
            // Tilt keeps a full-band detector, so no sidechain rebuild applies.
            DynEqShape::Tilt => {}
        }
    }

    pub(super) fn rebuild_eq_filters(&mut self, sample_rate: impl Into<f64>) {
        let sample_rate = sample_rate.into();
        self.frequency = self.frequency.clamp(20.0, Self::max_frequency(sample_rate));
        match self.shape {
            DynEqShape::Peak => {
                self.non_peak_coefficients = None;
                // Keep this exact Peak constructor path for legacy compatibility.
                for eq in &mut self.eq_filters {
                    *eq = Biquad::new(
                        BiquadFilterType::Peak,
                        self.frequency as f64,
                        sample_rate,
                        self.q as f64,
                        self.target_gain_db as f64,
                    );
                }
            }
            DynEqShape::Tilt => {
                self.non_peak_coefficients = design_tilt_coefficients(
                    self.frequency as f64,
                    sample_rate,
                    self.target_gain_db as f64,
                );
                // These Biquads carry per-channel recurrence state. Tilt
                // coefficients are supplied by process_with_coefficients.
                for eq in &mut self.eq_filters {
                    *eq = Biquad::new(
                        BiquadFilterType::Peak,
                        self.frequency as f64,
                        sample_rate,
                        self.q as f64,
                        0.0,
                    );
                }
            }
            shape => {
                self.non_peak_coefficients = design_shelf_coefficients(
                    shape,
                    self.frequency as f64,
                    sample_rate,
                    self.target_gain_db as f64,
                    self.shelf_slope as f64,
                );
                // These Biquads carry per-channel recurrence state. Shelf
                // coefficients are supplied by process_with_coefficients.
                for eq in &mut self.eq_filters {
                    *eq = Biquad::new(
                        BiquadFilterType::Peak,
                        self.frequency as f64,
                        sample_rate,
                        self.q as f64,
                        0.0,
                    );
                }
            }
        }
    }

    #[inline]
    pub(super) fn process_eq(&mut self, channel: usize, input: f64) -> f64 {
        if self.shape == DynEqShape::Peak {
            self.eq_filters[channel].process(input)
        } else if let Some(coefficients) = &self.non_peak_coefficients {
            self.eq_filters[channel].process_with_coefficients(input, coefficients)
        } else {
            // Strict constructors validate the shelf/tilt design before use.
            // Keep infallible legacy construction finite if malformed state
            // slips in.
            input
        }
    }

    pub(super) fn reset(&mut self, sample_rate: impl Into<f64>) {
        let sample_rate = sample_rate.into();
        self.rebuild_sidechain_filters(sample_rate);
        self.rebuild_eq_filters(sample_rate);
        for core in &mut self.cores {
            core.reset();
        }
    }

    pub(super) fn get_effective_threshold(&self, global_threshold: f32) -> f32 {
        if self.use_band_threshold {
            self.band_threshold
        } else {
            global_threshold
        }
    }

    pub(super) fn get_effective_ratio(&self, global_ratio: f32) -> f32 {
        if self.use_band_ratio {
            self.band_ratio
        } else {
            global_ratio
        }
    }
}

/// Build the normalized RBJ/W3C shelf section on the control thread.
/// `None` means invalid or unstable inputs/coefficients.
pub(super) fn design_shelf_coefficients(
    shape: DynEqShape,
    frequency: f64,
    sample_rate: f64,
    gain_db: f64,
    slope: f64,
) -> Option<BiquadCoefficients<f64>> {
    if !matches!(shape, DynEqShape::LowShelf | DynEqShape::HighShelf)
        || !frequency.is_finite()
        || !sample_rate.is_finite()
        || !gain_db.is_finite()
        || !slope.is_finite()
        || sample_rate <= 0.0
        || !(0.0..sample_rate * 0.5).contains(&frequency)
        || !(-24.0..=24.0).contains(&gain_db)
        || !(0.1..=1.0).contains(&slope)
    {
        return None;
    }

    let amplitude = 10.0_f64.powf(gain_db / 40.0);
    let omega = 2.0 * std::f64::consts::PI * frequency / sample_rate;
    let cosine = omega.cos();
    let alpha = omega.sin()
        * 0.5
        * (((amplitude + amplitude.recip()) * (slope.recip() - 1.0)) + 2.0).sqrt();
    let beta = 2.0 * amplitude.sqrt() * alpha;

    let (b0, b1, b2, a0, a1, a2) = match shape {
        DynEqShape::LowShelf => (
            amplitude * ((amplitude + 1.0) - (amplitude - 1.0) * cosine + beta),
            2.0 * amplitude * ((amplitude - 1.0) - (amplitude + 1.0) * cosine),
            amplitude * ((amplitude + 1.0) - (amplitude - 1.0) * cosine - beta),
            (amplitude + 1.0) + (amplitude - 1.0) * cosine + beta,
            -2.0 * ((amplitude - 1.0) + (amplitude + 1.0) * cosine),
            (amplitude + 1.0) + (amplitude - 1.0) * cosine - beta,
        ),
        DynEqShape::HighShelf => (
            amplitude * ((amplitude + 1.0) + (amplitude - 1.0) * cosine + beta),
            -2.0 * amplitude * ((amplitude - 1.0) + (amplitude + 1.0) * cosine),
            amplitude * ((amplitude + 1.0) + (amplitude - 1.0) * cosine - beta),
            (amplitude + 1.0) - (amplitude - 1.0) * cosine + beta,
            2.0 * ((amplitude - 1.0) - (amplitude + 1.0) * cosine),
            (amplitude + 1.0) - (amplitude - 1.0) * cosine - beta,
        ),
        // Peak and Tilt never reach this shelf-only match; the guard above
        // rejects them first.
        _ => return None,
    };

    if !a0.is_finite() || a0 <= 0.0 {
        return None;
    }
    let coefficients = BiquadCoefficients {
        b0: b0 / a0,
        b1: b1 / a0,
        b2: b2 / a0,
        a1: a1 / a0,
        a2: a2 / a0,
    };
    if [
        coefficients.b0,
        coefficients.b1,
        coefficients.b2,
        coefficients.a1,
        coefficients.a2,
    ]
    .iter()
    .any(|coefficient| !coefficient.is_finite())
    {
        return None;
    }
    // Jury conditions for a real, normalized second-order denominator.
    if coefficients.a2.abs() >= 1.0
        || 1.0 + coefficients.a1 + coefficients.a2 <= 0.0
        || 1.0 - coefficients.a1 + coefficients.a2 <= 0.0
    {
        return None;
    }
    Some(coefficients)
}

/// Design the first-order pivot tilt section on the control thread.
///
/// The analog prototype is `H(s) = (s + g*w0) / (g*s + w0)` with
/// `g = 10^(gain_db/20)`, giving exactly `gain_db` at DC, `-gain_db` at
/// Nyquist, and 0 dB at the pivot. The pivot is prewarped
/// (`w0 = 2*fs*tan(pi*pivot/fs)`) so the digital section keeps unity gain
/// at the pivot. `None` means invalid or unstable inputs/coefficients.
pub(super) fn design_tilt_coefficients(
    pivot_hz: f64,
    sample_rate: f64,
    gain_db: f64,
) -> Option<BiquadCoefficients<f64>> {
    if !pivot_hz.is_finite()
        || !sample_rate.is_finite()
        || !gain_db.is_finite()
        || sample_rate <= 0.0
        || !(0.0..sample_rate * 0.5).contains(&pivot_hz)
        || !(-24.0..=24.0).contains(&gain_db)
    {
        return None;
    }

    let gain = 10.0_f64.powf(gain_db / 20.0);
    let warped = 2.0 * sample_rate * (std::f64::consts::PI * pivot_hz / sample_rate).tan();
    let bilinear = 2.0 * sample_rate;
    let a0 = gain * bilinear + warped;
    if !a0.is_finite() || a0 <= 0.0 || !warped.is_finite() || warped <= 0.0 {
        return None;
    }
    let coefficients = BiquadCoefficients {
        b0: (bilinear + gain * warped) / a0,
        b1: (gain * warped - bilinear) / a0,
        b2: 0.0,
        a1: (warped - gain * bilinear) / a0,
        a2: 0.0,
    };
    if [coefficients.b0, coefficients.b1, coefficients.a1]
        .iter()
        .any(|coefficient| !coefficient.is_finite())
    {
        return None;
    }
    // First-order stability: the single pole must sit inside the unit circle.
    // The construction above guarantees this for positive pivot and rate;
    // the check below keeps malformed inputs finite regardless.
    if coefficients.a1.abs() >= 1.0 {
        return None;
    }
    Some(coefficients)
}
