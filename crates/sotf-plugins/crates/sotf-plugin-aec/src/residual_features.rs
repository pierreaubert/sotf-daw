//! Causal feature contract for optional residual echo gain estimation.
//!
//! Analysis uses the existing 512-point AEC spectra at 256-sample endpoints.
//! Sixteen disjoint linear-bin bands cover DC through Nyquist. All arithmetic
//! before bounded feature conversion uses f64. Invalid observations reset the
//! estimator and produce no proposal. This module does not modify adaptive state.

// Rust guideline compliant 2026-02-21
use rustfft::num_complex::Complex;

/// Number of gain bands covering all 257 unique spectral bins.
pub const BANDS: usize = 16;
/// Number of bounded, versioned features per gain band.
pub const FEATURES: usize = 12;
/// Fixed AEC analysis transform size.
pub const FFT_SIZE: usize = 512;
/// Unique real-transform bins, including DC and Nyquist.
pub const BINS: usize = FFT_SIZE / 2 + 1;
/// One causal endpoint's bounded features; no heap-owned memory.
pub type FeatureFrame = [[f32; FEATURES]; BANDS];

/// Native AEC state exposed only for explicit QA corpus collection.
#[cfg(feature = "qa")]
#[derive(Debug)]
pub struct TrainingAnalysis<'a> {
    /// Current foreground error, before any residual post-filter.
    pub error: &'a [f32],
    /// FFT of `[zeros(256), current foreground error]`.
    pub error_spectrum: &'a [Complex<f32>],
    /// Existing overlap-save foreground echo estimate spectrum.
    pub echo_spectrum: &'a [Complex<f32>],
    /// Current far-end reference block, before input storage is reused.
    pub reference: &'a [f32],
    /// Effective real wet-branch spectral gains, before the outer wet/dry ramp.
    ///
    /// Conditional source decomposition holds these decisions fixed. A
    /// reconstruction using this array also requires the wet mix to be one.
    pub wet_gains: &'a [f32],
}

/// Return a band's half-open unique-bin range.
pub fn band_range(band: usize) -> std::ops::Range<usize> {
    let band = band.min(BANDS - 1);
    band * BINS / BANDS..(band + 1) * BINS / BANDS
}

#[derive(Debug, Clone, Copy, Default)]
struct Moments {
    error: f64,
    echo: f64,
    cross: Complex<f64>,
}

impl Moments {
    fn observe(&mut self, error: Complex<f32>, echo: Complex<f32>, weight: f64) {
        let error = Complex::new(f64::from(error.re), f64::from(error.im));
        let echo = Complex::new(f64::from(echo.re), f64::from(echo.im));
        self.error += weight * error.norm_sqr();
        self.echo += weight * echo.norm_sqr();
        self.cross += weight * error * echo.conj();
    }

    fn coherence(self) -> f32 {
        (self.cross.norm_sqr() / (self.error * self.echo).max(1e-60)).clamp(0.0, 1.0) as f32
    }
}

fn level(power: f64) -> f32 {
    ((10.0 * power.max(1e-12).log10()).clamp(-120.0, 0.0) / 120.0 + 1.0) as f32
}

fn ratio(numerator: f64, denominator: f64) -> f32 {
    ((10.0 * (numerator.max(1e-12) / denominator.max(1e-12)).log10()).clamp(-40.0, 40.0) / 80.0
        + 0.5) as f32
}

/// Fixed-storage causal band statistics with a 50 ms physical smoothing time.
#[derive(Debug)]
pub struct ResidualFeatureExtractor {
    sample_rate: f64,
    alpha: f64,
    smoothed: [Moments; BANDS],
}

impl ResidualFeatureExtractor {
    /// Prepare analysis for a supported speech-band device rate.
    ///
    /// # Errors
    /// Rejects non-finite rates and rates outside 16–48 kHz.
    pub fn new(sample_rate: f64) -> Result<Self, String> {
        if !sample_rate.is_finite() || !(16_000.0..=48_000.0).contains(&sample_rate) {
            return Err("Learned residual analysis requires 16..48 kHz".into());
        }
        Ok(Self {
            sample_rate,
            alpha: (-256.0 / sample_rate / 0.050).exp(),
            smoothed: [Moments::default(); BANDS],
        })
    }

    /// Forget prior stream observations without allocation.
    pub fn reset(&mut self) {
        self.smoothed.fill(Moments::default());
    }

    /// Analyze one completed native block, returning no proposal for invalid data.
    ///
    /// Power arguments are mean-square current microphone and reference samples.
    /// Spectra must contain exactly the 257 unique bins of the native transform.
    /// No allocation, model execution, adaptive update or audio mutation occurs.
    pub fn observe(
        &mut self,
        error: &[Complex<f32>],
        echo: &[Complex<f32>],
        mic_power: f64,
        reference_power: f64,
    ) -> Option<FeatureFrame> {
        if error.len() != BINS
            || echo.len() != BINS
            || !mic_power.is_finite()
            || !reference_power.is_finite()
            || mic_power < 0.0
            || reference_power < 0.0
            || error
                .iter()
                .chain(echo)
                .any(|x| !x.re.is_finite() || !x.im.is_finite())
        {
            self.reset();
            return None;
        }
        let mut global = Moments::default();
        for (k, (&e, &h)) in error.iter().zip(echo).enumerate() {
            global.observe(e, h, if k == 0 || k == BINS - 1 { 1.0 } else { 2.0 });
        }
        let mut frame = [[0.0; FEATURES]; BANDS];
        for (band, out) in frame.iter_mut().enumerate() {
            let range = band_range(band);
            let center_bin = (range.start + range.end - 1) as f64 * 0.5;
            let mut current = Moments::default();
            for k in range {
                current.observe(
                    error[k],
                    echo[k],
                    if k == 0 || k == BINS - 1 { 1.0 } else { 2.0 },
                );
            }
            let old = self.smoothed[band];
            let alpha = self.alpha;
            let smoothed = Moments {
                error: alpha * old.error + (1.0 - alpha) * current.error,
                echo: alpha * old.echo + (1.0 - alpha) * current.echo,
                cross: alpha * old.cross + (1.0 - alpha) * current.cross,
            };
            self.smoothed[band] = smoothed;
            let scale = (FFT_SIZE * FFT_SIZE) as f64;
            *out = [
                (center_bin * self.sample_rate / FFT_SIZE as f64 / 24_000.0) as f32,
                level(current.error / scale),
                level(current.echo / scale),
                ratio(current.error / scale, current.echo / scale),
                ratio(smoothed.error / scale, smoothed.echo / scale),
                current.coherence(),
                smoothed.coherence(),
                ratio(global.error / scale, global.echo / scale),
                global.coherence(),
                level(reference_power),
                ratio(mic_power, reference_power),
                (self.sample_rate / 48_000.0) as f32,
            ];
        }
        Some(frame)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_unique_bin_belongs_to_exactly_one_band() {
        let bins: Vec<usize> = (0..BANDS).flat_map(band_range).collect();
        assert_eq!(bins, (0..BINS).collect::<Vec<_>>());
    }

    #[test]
    fn coherence_and_physical_power_follow_independent_equations() {
        let error = [Complex::new(2.0, 0.0); BINS];
        let echo = [Complex::new(0.0, 1.0); BINS];
        let frame = ResidualFeatureExtractor::new(48_000.0)
            .unwrap()
            .observe(&error, &echo, 0.01, 0.04)
            .unwrap();
        let expected_ratio = (10.0_f64 * 4.0_f64.log10() / 80.0 + 0.5) as f32;
        assert!((frame[4][3] - expected_ratio).abs() < 1e-6);
        assert_eq!(frame[4][5], 1.0);
        assert_eq!(frame[4][6], 1.0);
        assert!((frame[4][9] - (10.0 * 0.04_f32.log10() / 120.0 + 1.0)).abs() < 1e-6);
    }

    #[test]
    fn invalid_observations_retire_history_and_reset_is_exact() {
        let good = [Complex::new(0.1, 0.2); BINS];
        let mut dirty = ResidualFeatureExtractor::new(16_000.0).unwrap();
        dirty.observe(&good, &good, 0.1, 0.1).unwrap();
        let mut bad = good;
        bad[BINS - 1].im = f32::NAN;
        assert!(dirty.observe(&good, &bad, 0.1, 0.1).is_none());
        let mut fresh = ResidualFeatureExtractor::new(16_000.0).unwrap();
        assert_eq!(
            dirty.observe(&good, &good, 0.1, 0.1),
            fresh.observe(&good, &good, 0.1, 0.1)
        );
        assert!(dirty.observe(&good, &good, -1.0, 0.0).is_none());
    }

    #[cfg(feature = "qa")]
    #[test]
    fn qa_view_exposes_only_completed_live_native_blocks_and_retires_at_eof() {
        use sotf_host::{Plugin, ProcessContext};
        let mut plugin =
            crate::AecPlugin::from_params(48_000, crate::AecPluginParams::default()).unwrap();
        assert!(plugin.residual_training_analysis().is_none());
        let input: Vec<f32> = (0..128).flat_map(|_| [0.1, 0.0]).collect();
        let mut output = [0.0; 128];
        plugin
            .process(&input, &mut output, &ProcessContext::new(48_000, 128))
            .unwrap();
        assert!(plugin.residual_training_analysis().is_none());
        plugin
            .process(&input, &mut output, &ProcessContext::new(48_000, 128))
            .unwrap();
        let view = plugin.residual_training_analysis().unwrap();
        assert_eq!(view.error, &[0.1; 256]);
        assert_eq!(view.reference, &[0.0; 256]);
        assert_eq!(view.error_spectrum.len(), BINS);
        plugin
            .drain(&mut output, &ProcessContext::new(48_000, 128))
            .unwrap();
        assert!(plugin.residual_training_analysis().is_none());
        plugin.reset();
        assert!(plugin.residual_training_analysis().is_none());
    }
}
