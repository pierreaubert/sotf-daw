//! Shared experimental music/noise target-mask evaluation with fixed storage.
//!
//! A validated model may replace synthesis gains for confident interior bins.
//! Classical gate/envelope histories keep running independently. This QA-only
//! evaluator has no factory registration; synthetic assets remain unadmitted.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use sha2::{Digest, Sha256};

const BINS: usize = 513;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModel {
    format: String,
    weights: [[f64; 8]; 6],
    bias: [f64; 8],
    output_weights: [f64; 8],
    output_bias: f64,
    provenance: String,
    deployment_admitted: bool,
}

/// Fixed-storage six-feature, eight-hidden-unit gain estimator.
#[derive(Debug)]
pub struct DenoisingMask {
    weights: [[f64; 8]; 6],
    bias: [f64; 8],
    output_weights: [f64; 8],
    output_bias: f64,
}

impl DenoisingMask {
    /// Load bounded coefficients with exact identity and experimental provenance.
    ///
    /// # Errors
    /// Rejects oversized input, hash mismatch, unknown contract or invalid coefficients.
    pub fn from_json(bytes: &[u8], sha: &str) -> Result<Self, String> {
        if bytes.is_empty() || bytes.len() > 65536 || sha.len() != 64 {
            return Err("invalid denoising model size/hash".into());
        }
        let digest = Sha256::digest(bytes);
        let hex = b"0123456789abcdef";
        if !digest
            .iter()
            .zip(sha.as_bytes().as_chunks::<2>().0)
            .all(|(&v, pair)| *pair == [hex[usize::from(v >> 4)], hex[usize::from(v & 15)]])
        {
            return Err("denoising model identity mismatch".into());
        }
        let model: RawModel = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        if model.format != "sotf-expander-denoising-mask-v1"
            || model.provenance
                != "project-generated music/noise; GPL-3.0-or-later; no external audio"
            || model.deployment_admitted
            || model
                .weights
                .iter()
                .flatten()
                .chain(&model.bias)
                .chain(&model.output_weights)
                .chain(std::iter::once(&model.output_bias))
                .any(|v| !v.is_finite() || v.abs() > 16.0)
        {
            return Err("unsupported experimental denoising model".into());
        }
        Ok(Self {
            weights: model.weights,
            bias: model.bias,
            output_weights: model.output_weights,
            output_bias: model.output_bias,
        })
    }

    /// Predict a target Wiener amplitude ratio from normalized causal features.
    pub fn predict(&self, features: &[f64; 6]) -> Option<f32> {
        if features
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return None;
        }
        let mut logit = self.output_bias;
        for j in 0..8 {
            let z = self.bias[j]
                + features
                    .iter()
                    .enumerate()
                    .map(|(k, x)| (2.0 * x - 1.0) * self.weights[k][j])
                    .sum::<f64>();
            logit += z.tanh() * self.output_weights[j];
        }
        let value = if logit >= 0.0 {
            1.0 / (1.0 + (-logit).exp())
        } else {
            let e = logit.exp();
            e / (1.0 + e)
        };
        value.is_finite().then_some(value as f32)
    }
}

/// Causal normalized features for the fixed 1024-point linked spectrum.
///
/// Magnitudes are the maximum across channels, divided by the Hann window sum
/// and multiplied by two. Previous evidence is a 30 ms smoothed power history.
/// Returns no row for silence or malformed/overloaded evidence.
pub fn features(power: &[f64], previous: &[f64], bin: usize) -> Option<[f64; 6]> {
    if power.len() != BINS
        || previous.len() != BINS
        || bin >= BINS
        || power
            .iter()
            .chain(previous)
            .any(|x| !x.is_finite() || *x < 0.0 || *x > 4096.0)
    {
        return None;
    }
    let mean = power.iter().sum::<f64>() / BINS as f64;
    features_valid(power, previous, bin, mean)
}

fn features_valid(power: &[f64], previous: &[f64], bin: usize, mean: f64) -> Option<[f64; 6]> {
    if mean <= 1e-20 {
        return None;
    }
    let at = |offset: isize| power[bin.saturating_add_signed(offset).min(BINS - 1)].max(1e-20);
    let local = (-4..=4).map(at).sum::<f64>() / 9.0;
    let geometric = ((-4..=4).map(|i| at(i).ln()).sum::<f64>() / 9.0).exp();
    let p = power[bin].max(1e-20);
    Some([
        ((10.0 * p.log10() + 100.0) / 100.0).clamp(0.0, 1.0),
        ((10.0 * (p / local).log10() + 20.0) / 40.0).clamp(0.0, 1.0),
        (geometric / local).clamp(0.0, 1.0),
        ((10.0 * (p / previous[bin].max(1e-20)).log10() + 30.0) / 60.0).clamp(0.0, 1.0),
        bin as f64 / 512.0,
        ((10.0 * mean.log10() + 100.0) / 100.0).clamp(0.0, 1.0),
    ])
}

/// Counts source hops and linked bins, excluding frozen EOF continuation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize)]
pub struct DenoisingCounters {
    /// Source hops, including causal startup prefix windows.
    pub source_hops: u64,
    /// Interior bins with confident model selection.
    pub selected_bins: u64,
    /// Unsupported, uncertain, invalid and boundary bins using classical gain.
    pub fallback_bins: u64,
}

/// Preallocated causal mask selection shared by explicit QA restoration modes.
pub struct DenoisingState {
    model: Option<DenoisingMask>,
    previous: [f64; BINS],
    power: [f64; BINS],
    smoothed: [f32; BINS],
    selected: [Option<f32>; BINS],
    counters: DenoisingCounters,
    draining: bool,
}

impl std::fmt::Debug for DenoisingState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DenoisingState")
            .field("has_model", &self.model.is_some())
            .field("counters", &self.counters)
            .field("draining", &self.draining)
            .finish_non_exhaustive()
    }
}

impl DenoisingState {
    /// Prepare fixed storage with an optional validated candidate.
    pub fn new(model: Option<DenoisingMask>) -> Self {
        Self {
            model,
            previous: [0.0; BINS],
            power: [0.0; BINS],
            smoothed: [1.0; BINS],
            selected: [None; BINS],
            counters: DenoisingCounters::default(),
            draining: false,
        }
    }

    /// Clear learned history and counters while retaining the model.
    pub fn reset(&mut self) {
        self.previous.fill(0.0);
        self.power.fill(0.0);
        self.smoothed.fill(1.0);
        self.selected.fill(None);
        self.counters = DenoisingCounters::default();
        self.draining = false;
    }

    /// Analyze one source hop of linked unnormalized Hann FFT magnitudes.
    ///
    /// Requires513 bins, a positive Hann sum, and a supported44.1/48/96kHz clock.
    /// Invalid evidence selects exact caller-owned classical gains. Frozen EOF
    /// continuation changes neither selected gains nor source counters.
    pub fn prepare(&mut self, norms: &[f32], window_sum: f32, rate: f64) {
        if self.draining {
            return;
        }
        self.counters.source_hops += 1;
        self.selected.fill(None);
        let supported = [44100.0, 48000.0, 96000.0].contains(&rate)
            && norms.len() == BINS
            && window_sum.is_finite()
            && window_sum > 0.0
            && norms.iter().all(|v| v.is_finite() && *v >= 0.0);
        if !supported || self.model.is_none() {
            self.counters.fallback_bins += BINS as u64;
            return;
        }
        for (p, n) in self.power.iter_mut().zip(norms) {
            *p = (f64::from(*n) * 2.0 / f64::from(window_sum)).powi(2);
        }
        if self.power.iter().any(|p| *p > 4096.0) {
            self.counters.fallback_bins += BINS as u64;
            return;
        }
        let mean = self.power.iter().sum::<f64>() / BINS as f64;
        // Time constants are fixed qualification policy, evaluated at the 256-sample hop.
        let history = (-256.0 / (0.030 * rate)).exp();
        let attack = (-256.0 / (0.005 * rate)).exp() as f32;
        let release = (-256.0 / (0.030 * rate)).exp() as f32;
        for k in 1..BINS - 1 {
            let predicted = features_valid(&self.power, &self.previous, k, mean)
                .and_then(|row| self.model.as_ref().and_then(|model| model.predict(&row)));
            if let Some(p) = predicted.filter(|p| *p <= 0.15 || *p >= 0.85) {
                // Fixed -20 dB minimum amplitude gain, no boost or extra lookahead.
                let target = p.max(0.1);
                let coefficient = if target < self.smoothed[k] {
                    attack
                } else {
                    release
                };
                self.smoothed[k] = target + coefficient * (self.smoothed[k] - target);
                self.selected[k] = Some(self.smoothed[k]);
                self.counters.selected_bins += 1;
            }
        }
        self.counters.fallback_bins +=
            BINS as u64 - self.selected.iter().filter(|v| v.is_some()).count() as u64;
        for (previous, power) in self.previous.iter_mut().zip(self.power) {
            *previous = power + history * (*previous - power);
        }
    }

    /// Select a learned gain, retaining classical gain outside selected bins.
    pub fn gain(&self, bin: usize, classical: f32) -> f32 {
        self.selected
            .get(bin)
            .copied()
            .flatten()
            .unwrap_or(classical)
    }

    /// Freeze or resume learned updates for caller-owned finite EOF continuation.
    pub fn set_draining(&mut self, draining: bool) {
        self.draining = draining;
    }

    /// Copy linked source-hop/bin counters without allocation.
    pub fn counters(&self) -> DenoisingCounters {
        self.counters
    }
}
