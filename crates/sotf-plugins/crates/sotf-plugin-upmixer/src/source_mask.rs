//! Bounded QA-only direct/ambient masks on the existing analysis clock.
//!
//! Models load on the control thread. Inference uses fixed arrays and no worker,
//! queue or lookahead. Invalid evidence retains ordinary native decomposition.
//! This unadmitted synthetic model does not identify speech or physical sources.

// Rust guideline compliant 2026-02-21
use rustfft::num_complex::Complex;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Fixed source-model analysis geometry, independent of the native ERB bands.
pub const FFT_SIZE: usize = 512;
/// Eight-bin analysis groups exclude DC and Nyquist.
pub const BANDS: usize = 32;

/// Validated fixed-size direct/ambient source-mask network.
#[derive(Debug, Clone)]
pub struct SourceMaskModel {
    coefficients: [f64; 65],
    raw_power_policy: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Document {
    format: String,
    policy: serde_json::Value,
    provenance: serde_json::Value,
    deployment_admitted: bool,
    coefficients: Vec<f64>,
}

impl SourceMaskModel {
    /// Load the exact unadmitted model contract on the control thread.
    ///
    /// # Errors
    /// Rejects oversized bytes, wrong hashes, metadata, shapes or coefficient domains.
    pub fn from_json(bytes: &[u8], sha256: &str) -> Result<Self, String> {
        if bytes.len() > 65536 {
            return Err("source-mask size/hash mismatch".into());
        }
        let actual: String = Sha256::digest(bytes)
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect();
        if actual != sha256 {
            return Err("source-mask size/hash mismatch".into());
        }
        let doc: Document = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        let raw_power_policy = doc.format == "sotf-upmixer-direct-ambient-mask-v2";
        let mut policy = serde_json::json!({
            "sample_rates": [48000], "fft_size": 512, "hop_size": 256,
            "band_bins": 8, "features": 6, "hidden": 8,
            "window": "periodic_sqrt_hann", "model_lookahead_samples": 0,
            "label": "generated_direct_stem_power_fraction",
            "fallback": "analytic_directness", "mask_bounds": [0.0, 1.0]
        });
        let mut provenance = serde_json::json!({
            "origin": "project-generated panned direct and stereo diffuse stems",
            "license": "GPL-3.0-or-later", "recorded_sources": false,
            "speech_identity": false
        });
        if raw_power_policy {
            policy["fallback"] = serde_json::json!("ordinary_native_decomposition");
            policy["excitation"] = serde_json::json!({
                "band_power_interval_exclusive": [1e-16, 1e12],
                "relative_to_peak_band_min": 1e-6,
                "power_scale": "reference_fft",
                "dc_nyquist": "excluded"
            });
            provenance["training_features"] = serde_json::json!("native_f32_fft_f64_covariance");
        }
        if (!raw_power_policy && doc.format != "sotf-upmixer-direct-ambient-mask-v1")
            || doc.policy != policy
            || doc.provenance != provenance
            || doc.deployment_admitted
            || doc
                .coefficients
                .iter()
                .any(|v| !v.is_finite() || v.abs() > 16.0)
        {
            return Err("unsupported source-mask contract".into());
        }
        let coefficients = doc
            .coefficients
            .try_into()
            .map_err(|_| "source-mask needs65 coefficients")?;
        Ok(Self {
            coefficients,
            raw_power_policy,
        })
    }

    /// Infer a bounded direct fraction from six normalized features without allocation.
    pub fn predict(&self, features: &[f64; 6]) -> Option<f32> {
        if features
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return None;
        }
        let t = &self.coefficients;
        let mut z = t[64];
        for h in 0..8 {
            let mut hidden = t[48 + h];
            for (i, x) in features.iter().enumerate() {
                hidden += (2.0 * x - 1.0) * t[8 * i + h];
            }
            z += hidden.tanh() * t[56 + h];
        }
        Some((1.0 / (1.0 + (-z).exp())) as f32)
    }
}

/// Derive reference-scale covariance features from one non-boundary spectral band.
///
/// `power_scale` compensates fixed native FFT headroom:2 for the current native
/// analysis,1 for spectra from the independent Python reference.
pub fn band_features(
    left: &[Complex<f32>],
    right: &[Complex<f32>],
    start: usize,
    end: usize,
    power_scale: f64,
) -> Option<[f64; 6]> {
    band_observation(left, right, start, end, power_scale).and_then(|(_, features)| features)
}

// Keep unclipped power separate from the unchanged v1 feature-validity policy.
fn band_observation(
    left: &[Complex<f32>],
    right: &[Complex<f32>],
    start: usize,
    end: usize,
    power_scale: f64,
) -> Option<(f64, Option<[f64; 6]>)> {
    if start == 0
        || start >= end
        || end > FFT_SIZE / 2
        || left.len() < end
        || right.len() < end
        || !power_scale.is_finite()
        || power_scale <= 0.0
    {
        return None;
    }
    let (mut pl, mut pr, mut cross) = (0.0, 0.0, Complex::<f64>::new(0.0, 0.0));
    for k in start..end {
        let l = Complex::new(f64::from(left[k].re), f64::from(left[k].im));
        let r = Complex::new(f64::from(right[k].re), f64::from(right[k].im));
        pl += l.norm_sqr() * power_scale;
        pr += r.norm_sqr() * power_scale;
        cross += l * r.conj() * power_scale;
    }
    let total = pl + pr;
    if !total.is_finite() {
        return None;
    }
    if total <= 1e-16 || total >= 1e12 {
        return Some((total, None));
    }
    Some((
        total,
        Some([
            ((pl - pr).abs() / total).clamp(0.0, 1.0),
            ((2.0 * cross.re / total).clamp(-1.0, 1.0) + 1.0) * 0.5,
            (2.0 * cross.im.abs() / total).clamp(0.0, 1.0),
            (2.0 * cross.norm() / total).clamp(0.0, 1.0),
            (start + end - 1) as f64 / FFT_SIZE as f64,
            ((10.0 * (total / FFT_SIZE as f64).log10() + 100.0) / 100.0).clamp(0.0, 1.0),
        ]),
    ))
}

/// Current causal analysis observations and source-only selection counters.
#[derive(Debug, Clone, Serialize)]
pub struct SourceMaskSnapshot {
    pub analysis_frames: u64,
    pub selected_bands: u64,
    pub fallback_bands: u64,
    pub features: [Option<[f64; 6]>; BANDS],
    pub masks: [Option<f32>; BANDS],
    /// Unclipped reference-scale stereo power per band, including silent bands.
    ///
    /// Excludes DC/Nyquist and compensates native FFT headroom by two.
    /// `None` means unsupported or non-finite evidence. This diagnostic does
    /// not change v1 feature validity, masks, fallback, or audio routing.
    pub band_power: [Option<f64>; BANDS],
}
impl Default for SourceMaskSnapshot {
    fn default() -> Self {
        Self {
            analysis_frames: 0,
            selected_bands: 0,
            fallback_bands: 0,
            features: [None; BANDS],
            masks: [None; BANDS],
            band_power: [None; BANDS],
        }
    }
}

#[derive(Debug, Default)]
pub(crate) struct SourceMasks {
    pub(crate) model: Option<SourceMaskModel>,
    pub(crate) snapshot: SourceMaskSnapshot,
}
impl SourceMasks {
    pub(crate) fn update(
        &mut self,
        left: &[Complex<f32>],
        right: &[Complex<f32>],
        supported: bool,
    ) {
        let state = &mut self.snapshot;
        state.analysis_frames = state.analysis_frames.saturating_add(1);
        state.features.fill(None);
        state.masks.fill(None);
        state.band_power.fill(None);
        for band in 0..BANDS {
            if supported && self.model.is_some() {
                let start = 1 + 8 * band;
                if let Some((power, features)) =
                    band_observation(left, right, start, (start + 8).min(FFT_SIZE / 2), 2.0)
                {
                    state.band_power[band] = Some(power);
                    state.features[band] = features;
                }
            }
        }
        let peak_power = state
            .band_power
            .iter()
            .flatten()
            .copied()
            .fold(0.0, f64::max);
        for band in 0..BANDS {
            let eligible = self.model.as_ref().is_some_and(|model| {
                !model.raw_power_policy
                    || state.band_power[band].is_some_and(|power| power >= peak_power * 1e-6)
            });
            if eligible {
                state.masks[band] = state.features[band]
                    .as_ref()
                    .and_then(|f| self.model.as_ref()?.predict(f));
            }
            if state.masks[band].is_some() {
                state.selected_bands = state.selected_bands.saturating_add(1);
            } else {
                state.fallback_bands = state.fallback_bands.saturating_add(1);
            }
        }
    }
    pub(crate) fn mask(&self, bin: usize) -> Option<f32> {
        if bin == 0 || bin >= FFT_SIZE / 2 {
            return None;
        }
        self.snapshot.masks[(bin - 1) / 8]
    }
}

impl crate::UpmixerPlugin {
    /// Install an optional QA source model before accepted audio, on the control thread.
    ///
    /// Unsupported rates, FFTs, HR or scalar-ML modes keep native decomposition.
    /// Reset retains the model and clears its observations. No runtime selector is added.
    ///
    /// # Errors
    /// Rejects installation after accepted audio or once finite-stream draining starts.
    pub fn configure_source_masks(&mut self, model: Option<SourceMaskModel>) -> Result<(), String> {
        if self.drain.has_input || self.drain.remaining.is_some() {
            return Err("reset Upmixer before replacing its source-mask model".into());
        }
        self.source_masks = SourceMasks {
            model,
            snapshot: SourceMaskSnapshot::default(),
        };
        Ok(())
    }

    /// Copy fixed-size source observations without allocating on the audio thread.
    pub fn source_mask_snapshot(&self) -> SourceMaskSnapshot {
        self.source_masks.snapshot.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::{SourceMaskModel, SourceMasks, band_observation};
    use rustfft::num_complex::Complex;

    #[test]
    fn raw_power_retains_values_below_log_clipping_and_feature_validity_floors() {
        for amplitude in [0.0_f32, 1e-10, 1e-6, 0.25] {
            let left = [Complex::new(amplitude, 0.0); 9];
            let right = [Complex::new(0.0, amplitude); 9];
            let (power, features) = band_observation(&left, &right, 1, 9, 2.0).unwrap();
            let expected = 32.0 * f64::from(amplitude).powi(2);
            assert_eq!(power, expected);
            assert_eq!(features.is_some(), expected > 1e-16);
            if amplitude == 1e-6 {
                assert_eq!(features.unwrap()[5], 0.0);
                assert!(power > 0.0);
            }
        }
        let invalid = [Complex::new(f32::NAN, 0.0); 9];
        assert!(band_observation(&invalid, &invalid, 1, 9, 2.0).is_none());
    }

    #[test]
    fn v2_reliability_mask_tracks_actual_power_on_both_sides_of_cutoff() {
        let mut left = [Complex::new(0.0, 0.0); 257];
        let right = left;
        left[1..9].fill(Complex::new(1.0, 0.0));
        // Neighboring f32 amplitudes straddle the declared 60 dB power cutoff.
        let boundary = 0.001_f32;
        for amplitude in [f32::from_bits(boundary.to_bits() - 1), boundary] {
            left[9..17].fill(Complex::new(amplitude, 0.0));
            for guarded in [false, true] {
                let mut masks = SourceMasks {
                    model: Some(SourceMaskModel {
                        coefficients: [0.0; 65],
                        raw_power_policy: guarded,
                    }),
                    ..SourceMasks::default()
                };
                masks.update(&left, &right, true);
                let snapshot = &masks.snapshot;
                assert_eq!(snapshot.masks[0], Some(0.5));
                let expected = !guarded
                    || snapshot.band_power[1].unwrap() >= snapshot.band_power[0].unwrap() * 1e-6;
                assert_eq!(snapshot.masks[1].is_some(), expected);
                assert_eq!(
                    snapshot.masks[1].is_some(),
                    !guarded || amplitude == boundary
                );
                assert!(snapshot.features[1].is_some());
                assert!(snapshot.masks[2..].iter().all(Option::is_none));
                assert_eq!(snapshot.selected_bands + snapshot.fallback_bands, 32);
            }
        }
    }
}
