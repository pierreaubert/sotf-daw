//! Bounded QA-only spatial noise mask with explicit provenance and hash checks.
//!
//! This model estimates target-direction/noise energy from one array spectrum.
//! It has no future context, speech identity inference or production admission.

// Rust guideline compliant 2026-02-21
use nalgebra::Complex;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Fixed input feature count.
pub const FEATURES: usize = 6;
/// Fixed hidden width bounds inference work per bin.
pub const HIDDEN: usize = 8;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMask {
    format: String,
    features: usize,
    hidden: usize,
    weights: [[f64; HIDDEN]; FEATURES],
    bias: [f64; HIDDEN],
    output_weights: [f64; HIDDEN],
    output_bias: f64,
    policy: Value,
    deployment_admitted: bool,
    provenance: Value,
}

/// Unadmitted fixed-size neural mask prepared on the control thread.
#[derive(Debug)]
pub struct NoiseMask {
    weights: [[f64; HIDDEN]; FEATURES],
    bias: [f64; HIDDEN],
    output_weights: [f64; HIDDEN],
    output_bias: f64,
}

impl NoiseMask {
    /// Validate bounded JSON, its explicit SHA256 and the fixed research policy.
    ///
    /// # Errors
    /// Rejects mismatched hash, dimensions, provenance, policy or finite bounds.
    pub fn from_json(bytes: &[u8], expected_sha256: &str) -> Result<Self, String> {
        if bytes.is_empty()
            || bytes.len() > 65_536
            || expected_sha256.len() != 64
            || !Sha256::digest(bytes)
                .iter()
                .zip(expected_sha256.as_bytes().as_chunks::<2>().0)
                .all(|(&byte, pair)| {
                    let hex = b"0123456789abcdef";
                    pair[0] == hex[usize::from(byte >> 4)] && pair[1] == hex[usize::from(byte & 15)]
                })
        {
            return Err("spatial mask size/hash mismatch".into());
        }
        let raw: RawMask = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        let policy = json!({"confidence":0.85,"maximum_added_lookahead_samples":0,
            "fallback":"native_frame_coherence_gate","sample_rates":[16000,48000,96000],
            "minimum_mics":2,"maximum_mics":8,"minimum_spacing_cm":1.0,
            "maximum_spacing_cm":10.0,"maximum_abs_steer_degrees":60.0});
        if raw.format != "sotf-beamformer-spatial-mask-v1"
            || raw.features != FEATURES
            || raw.hidden != HIDDEN
            || raw.deployment_admitted
            || raw.policy != policy
            || raw.provenance
                != json!({"origin":"project-generated complex plane-wave mixtures",
                "license":"GPL-3.0-or-later","recorded_speech":false})
            || raw
                .weights
                .iter()
                .flatten()
                .chain(&raw.bias)
                .chain(&raw.output_weights)
                .chain(std::iter::once(&raw.output_bias))
                .any(|v| !v.is_finite() || v.abs() > 16.0)
        {
            return Err("unsupported unadmitted spatial mask contract".into());
        }
        Ok(Self {
            weights: raw.weights,
            bias: raw.bias,
            output_weights: raw.output_weights,
            output_bias: raw.output_bias,
        })
    }

    /// Test the declared generated training geometry/rate domain.
    pub fn supports_context(rate: f64, mics: usize, spacing_cm: f32, angle: f32) -> bool {
        [16_000.0, 48_000.0, 96_000.0].contains(&rate)
            && (2..=8).contains(&mics)
            && (1.0..=10.0).contains(&spacing_cm)
            && angle.is_finite()
            && angle.abs() <= 60.0
    }

    /// Infer noise-energy probability from six bounded features without allocation.
    ///
    /// Returns `None` for evidence outside the fixed feature domain. Probability
    /// is not a calibrated population confidence or a speech-recognition score.
    pub fn probability(&self, features: &[f64; FEATURES]) -> Option<f32> {
        if features
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return None;
        }
        let mut logit = self.output_bias;
        for j in 0..HIDDEN {
            let mut z = self.bias[j];
            for (k, value) in features.iter().enumerate() {
                z += (2.0 * value - 1.0) * self.weights[k][j];
            }
            logit += z.tanh() * self.output_weights[j];
        }
        let probability = if logit >= 0.0 {
            1.0 / (1.0 + (-logit).exp())
        } else {
            let v = logit.exp();
            v / (1.0 + v)
        };
        probability.is_finite().then_some(probability as f32)
    }
}

/// Extract causal scale-invariant spatial features for one physical steering bin.
///
/// Returns `None` for malformed/nonfinite or silent/overloaded evidence. The
/// frequency input is bin index divided by the Nyquist bin index.
pub fn spatial_features(
    channels: &[Vec<Complex<f32>>],
    steering: &[Complex<f32>],
    bin: usize,
    normalized_frequency: f64,
) -> Option<[f64; FEATURES]> {
    let m = channels.len();
    if !(2..=8).contains(&m)
        || steering.len() != m
        || !normalized_frequency.is_finite()
        || !(0.0..=1.0).contains(&normalized_frequency)
    {
        return None;
    }
    let mut aligned = [Complex::new(0.0_f64, 0.0); 8];
    let mut powers = [0.0_f64; 8];
    let mut total = 0.0;
    let mut sum = Complex::new(0.0, 0.0);
    for i in 0..m {
        let sample = *channels[i].get(bin)?;
        let d = steering[i];
        let x = Complex::new(f64::from(sample.re), f64::from(sample.im));
        let d = Complex::new(f64::from(d.re), f64::from(d.im));
        if !x.re.is_finite()
            || !x.im.is_finite()
            || !d.re.is_finite()
            || !d.im.is_finite()
            || (d.norm() - 1.0).abs() > 1e-4
        {
            return None;
        }
        aligned[i] = d.conj() * x;
        powers[i] = x.norm_sqr();
        total += powers[i];
        sum += aligned[i];
    }
    if !(1e-20..=1e12).contains(&total) || total == 1e-20 {
        return None;
    }
    let mut phase = 0.0;
    let mut pairs = 0;
    for i in 0..m {
        for j in 0..i {
            let denominator = (powers[i] * powers[j]).sqrt();
            if denominator > 1e-20 {
                phase += (aligned[i] * aligned[j].conj()).re / denominator;
            }
            pairs += 1;
        }
    }
    let minimum = powers[..m].iter().copied().fold(f64::INFINITY, f64::min);
    let maximum = powers[..m].iter().copied().fold(0.0, f64::max);
    Some(
        [
            sum.norm_sqr() / (m as f64 * total),
            (phase / pairs as f64 + 1.0) / 2.0,
            minimum / maximum,
            powers[0] / total,
            normalized_frequency,
            (m - 2) as f64 / 6.0,
        ]
        .map(|v| v.clamp(0.0, 1.0)),
    )
}
