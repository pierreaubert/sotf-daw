//! Bounded centered-source estimator; no dialogue or instrument identity claim.

// Rust guideline compliant 2026-02-21
use rustfft::num_complex::Complex;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    format: String,
    features: usize,
    hidden: usize,
    weights: [[f64; 8]; 6],
    bias: [f64; 8],
    output_weights: [f64; 8],
    output_bias: f64,
    policy: Value,
    provenance: Value,
    deployment_admitted: bool,
}

/// Fixed unadmitted model loaded and validated on the control thread.
#[derive(Debug)]
pub struct SourceMask {
    weights: [[f64; 8]; 6],
    bias: [f64; 8],
    output_weights: [f64; 8],
    output_bias: f64,
}

impl SourceMask {
    /// Validate exact dimensions, finite coefficients, policy, provenance and SHA256.
    ///
    /// # Errors
    /// Rejects unsupported or mismatched model bytes; no admission is granted.
    pub fn from_json(bytes: &[u8], sha: &str) -> Result<Self, String> {
        if bytes.is_empty()
            || bytes.len() > 65_536
            || sha.len() != 64
            || !Sha256::digest(bytes)
                .iter()
                .zip(sha.as_bytes().as_chunks::<2>().0)
                .all(|(&v, p)| {
                    let hex = b"0123456789abcdef";
                    p[0] == hex[usize::from(v >> 4)] && p[1] == hex[usize::from(v & 15)]
                })
        {
            return Err("centered-source model size/hash mismatch".into());
        }
        let d: Raw = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        let policy = json!({"sample_rates":[44100,48000,96000],"fft_size":512,"channels":2,"confidence":0.85,
            "gain_limit_db":6.0,"gain_smoothing_ms":5.0,"maximum_model_lookahead_samples":0,
            "fallback":"neutral_remix","label":"centered_foreground_mid_energy_fraction"});
        if d.format != "sotf-centered-source-mask-v1"
            || d.features != 6
            || d.hidden != 8
            || d.deployment_admitted
            || d.policy != policy
            || d.provenance
                != json!({"origin":"project-generated centered foreground and stereo bed mixtures","license":"GPL-3.0-or-later","recorded_sources":false,"speech_identity":false})
            || d.weights
                .iter()
                .flatten()
                .chain(&d.bias)
                .chain(&d.output_weights)
                .chain(std::iter::once(&d.output_bias))
                .any(|v| !v.is_finite() || v.abs() > 16.0)
        {
            return Err("unsupported unadmitted centered-source contract".into());
        }
        Ok(Self {
            weights: d.weights,
            bias: d.bias,
            output_weights: d.output_weights,
            output_bias: d.output_bias,
        })
    }

    /// Predict the centered-foreground mid-energy fraction from normalized evidence.
    pub fn predict(&self, x: &[f64; 6]) -> Option<f32> {
        if x.iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v)) {
            return None;
        }
        let mut z = self.output_bias;
        for (j, (&bias, &output)) in self.bias.iter().zip(&self.output_weights).enumerate() {
            let hidden = bias
                + x.iter()
                    .enumerate()
                    .map(|(k, v)| (2.0 * v - 1.0) * self.weights[k][j])
                    .sum::<f64>();
            z += hidden.tanh() * output;
        }
        let p = if z >= 0.0 {
            1.0 / (1.0 + (-z).exp())
        } else {
            let e = z.exp();
            e / (1.0 + e)
        };
        p.is_finite().then_some(p as f32)
    }
}

/// Extract stereo-swap-invariant features at one physical real-FFT bin.
///
/// Returns `None` for silent, nonfinite or overloaded evidence. The bounded
/// excitation domain is a software guard, not physical source calibration.
pub fn spatial_features(left: Complex<f32>, right: Complex<f32>, bin: usize) -> Option<[f64; 6]> {
    if bin > 256 {
        return None;
    }
    let l = Complex::new(f64::from(left.re), f64::from(left.im));
    let r = Complex::new(f64::from(right.re), f64::from(right.im));
    if !l.re.is_finite() || !l.im.is_finite() || !r.re.is_finite() || !r.im.is_finite() {
        return None;
    }
    let pl = l.norm_sqr();
    let pr = r.norm_sqr();
    let total = pl + pr;
    if !(1e-20..=1e12).contains(&total) || total == 1e-20 {
        return None;
    }
    let cross = l * r.conj();
    let denominator = (pl * pr).sqrt();
    let (agreement, quadrature) = if denominator > 1e-20 {
        (cross.re / denominator, cross.im.abs() / denominator)
    } else {
        (0.0, 0.0)
    };
    Some(
        [
            (l + r).norm_sqr() / (2.0 * total),
            (agreement + 1.0) / 2.0,
            pl.min(pr) / pl.max(pr),
            quadrature,
            bin as f64 / 256.0,
            (10.0 * (total / 512.0).log10() + 100.0) / 100.0,
        ]
        .map(|v| v.clamp(0.0, 1.0)),
    )
}
