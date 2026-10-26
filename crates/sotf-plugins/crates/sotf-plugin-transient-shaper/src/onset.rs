//! Explicit causal onset assistance through bounded native attack modulation.
//!
//! The native detector always runs independently. Missing, uncertain, silent,
//! invalid or unsupported evidence returns the exact native attack control.
//! No trained asset or production selector is admitted by this QA-only module.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Raw {
    format: String,
    weights: [[f64; 8]; 6],
    bias: [f64; 8],
    output_weights: [f64; 8],
    output_bias: f64,
    policy: Value,
    provenance: String,
    deployment_admitted: bool,
}

/// Fixed six-feature, eight-hidden-unit classifier for explicit onset assistance.
#[derive(Debug)]
pub struct OnsetModel {
    weights: [[f64; 8]; 6],
    bias: [f64; 8],
    output_weights: [f64; 8],
    output_bias: f64,
}

impl OnsetModel {
    /// Validate bounded coefficients, identity and the exact experimental policy.
    ///
    /// # Errors
    /// Rejects oversized input, hash mismatch, unknown policy/provenance or invalid coefficients.
    pub fn from_json(bytes: &[u8], sha: &str) -> Result<Self, String> {
        if bytes.is_empty() || bytes.len() > 65536 || sha.len() != 64 {
            return Err("invalid onset model size/hash".into());
        }
        let hex = b"0123456789abcdef";
        if !Sha256::digest(bytes)
            .iter()
            .zip(sha.as_bytes().as_chunks::<2>().0)
            .all(|(&v, p)| *p == [hex[usize::from(v >> 4)], hex[usize::from(v & 15)]])
        {
            return Err("onset model identity mismatch".into());
        }
        let raw: Raw = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        let policy = json!({"sample_rates":[44100,48000,96000],"analysis_ms":2.0,"control_smoothing_ms":5.0,
            "maximum_attack_modulation":0.25,"confidence":0.9,"added_lookahead_samples":0,
            "fallback":"independent_native_attack","label":"causal_hop_contains_generated_onset"});
        if raw.format != "sotf-transient-onset-assist-v1"
            || raw.policy != policy
            || raw.deployment_admitted
            || raw.provenance
                != "project-generated onset/steady/noise signals; GPL-3.0-or-later; no external audio"
            || raw
                .weights
                .iter()
                .flatten()
                .chain(&raw.bias)
                .chain(&raw.output_weights)
                .chain(std::iter::once(&raw.output_bias))
                .any(|v| !v.is_finite() || v.abs() > 16.0)
        {
            return Err("unsupported experimental onset contract".into());
        }
        Ok(Self {
            weights: raw.weights,
            bias: raw.bias,
            output_weights: raw.output_weights,
            output_bias: raw.output_bias,
        })
    }

    /// Predict from normalized causal features; invalid evidence yields no result.
    pub fn predict(&self, x: &[f64; 6]) -> Option<f32> {
        if x.iter().any(|v| !v.is_finite() || !(0.0..=1.0).contains(v)) {
            return None;
        }
        let mut z = self.output_bias;
        for j in 0..8 {
            let h = self.bias[j]
                + x.iter()
                    .enumerate()
                    .map(|(k, x)| (2.0 * x - 1.0) * self.weights[k][j])
                    .sum::<f64>();
            z += h.tanh() * self.output_weights[j];
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

/// Extract causal hop statistics without a waveform buffer.
///
/// Features are level, crest, energy change, native transient ratio, rectified
/// amplitude variation and fast-envelope/RMS contrast. Native envelopes are
/// supplied read-only; no learned decision feeds either detector.
pub fn features(
    mean_power: f64,
    peak: f64,
    previous_power: f64,
    mean_difference: f64,
    mean_amplitude: f64,
    fast: f64,
    slow: f64,
) -> Option<[f64; 6]> {
    let values = [
        mean_power,
        peak,
        previous_power,
        mean_difference,
        mean_amplitude,
        fast,
        slow,
    ];
    if values.iter().any(|v| !v.is_finite() || *v < 0.0)
        || mean_power <= 1e-20
        || [mean_power, peak, previous_power, mean_amplitude, fast, slow]
            .iter()
            .any(|v| *v > 1.0)
        || mean_difference > 2.0
        || mean_power > peak * peak + 1e-12
        || mean_amplitude > peak + 1e-12
    {
        return None;
    }
    let rms = mean_power.sqrt();
    Some([
        ((20.0 * rms.max(1e-20).log10() + 100.0) / 100.0).clamp(0.0, 1.0),
        (20.0 * (peak / rms).max(1.0).log10() / 24.0).clamp(0.0, 1.0),
        ((10.0 * (mean_power / previous_power.max(1e-20)).log10() + 30.0) / 60.0).clamp(0.0, 1.0),
        ((fast - slow).max(0.0) / slow.max(1e-10)).clamp(0.0, 1.0),
        (mean_difference / (2.0 * mean_amplitude.max(1e-20))).clamp(0.0, 1.0),
        ((20.0 * (fast / rms).max(1e-20).log10() + 24.0) / 48.0).clamp(0.0, 1.0),
    ])
}

/// Causal source-frame and completed-hop decisions, copied without allocation.
#[derive(Debug, Copy, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct OnsetCounters {
    /// Accepted source frames across rate epochs until explicit reset.
    pub frames: u64,
    /// Complete causal analysis windows.
    pub hops: u64,
    /// Confident generated-onset predictions.
    pub onset: u64,
    /// Confident generated-nononset predictions.
    pub nononset: u64,
    /// Predictions inside the uncalibrated confidence gap.
    pub uncertain: u64,
    /// Missing models or unsupported clocks.
    pub unavailable: u64,
    /// Silent, overloaded or invalid feature windows.
    pub invalid_or_silent: u64,
    /// Partial window frames discarded when the analysis clock changes.
    pub discarded_partial_frames: u64,
}

/// Diagnostic state without granting classifier or perceptual admission.
#[derive(Debug, Copy, Clone, serde::Serialize)]
pub struct OnsetSnapshot {
    /// Source and classification counters.
    pub counters: OnsetCounters,
    /// Current rounded two-millisecond analysis period.
    pub hop_frames: usize,
    /// Last complete causal feature row, absent for invalid/silent evidence.
    pub features: Option<[f64; 6]>,
    /// Last model prediction, absent during fallback.
    pub probability: Option<f32>,
    /// Attack multiplier before the final native [-1,1] control clamp.
    pub attack_multiplier: f32,
}

pub(crate) struct OnsetAssist {
    model: Option<OnsetModel>,
    rate: f64,
    hop: usize,
    phase: usize,
    power: f64,
    amplitude: f64,
    difference: f64,
    peak: f64,
    last_amplitude: f64,
    previous_power: f64,
    invalid: bool,
    previous_valid: bool,
    target: f32,
    factor: f32,
    selected: bool,
    coefficient: f32,
    counters: OnsetCounters,
    last_features: Option<[f64; 6]>,
    probability: Option<f32>,
    applied: f32,
}

impl OnsetAssist {
    pub(crate) fn new(model: Option<OnsetModel>, rate: f64) -> Self {
        let mut state = Self {
            model,
            rate: 0.0,
            hop: 1,
            phase: 0,
            power: 0.0,
            amplitude: 0.0,
            difference: 0.0,
            peak: 0.0,
            last_amplitude: 0.0,
            previous_power: 0.0,
            invalid: false,
            previous_valid: true,
            target: 1.0,
            factor: 1.0,
            selected: false,
            coefficient: 0.0,
            counters: OnsetCounters::default(),
            last_features: None,
            probability: None,
            applied: 1.0,
        };
        state.retime(rate);
        state
    }

    pub(crate) fn retime(&mut self, rate: f64) {
        if self.rate != rate {
            self.counters.discarded_partial_frames += self.phase as u64;
            self.clear_window();
            self.selected = false;
            self.target = 1.0;
            self.probability = None;
            self.last_features = None;
            self.previous_valid = self.rate == 0.0;
            self.applied = 1.0;
        }
        self.rate = rate;
        self.hop = (rate * 0.002).round().max(1.0) as usize;
        self.coefficient = (-1.0 / (0.005 * rate)).exp() as f32;
    }

    fn clear_window(&mut self) {
        self.phase = 0;
        self.power = 0.0;
        self.amplitude = 0.0;
        self.difference = 0.0;
        self.peak = 0.0;
        self.invalid = false;
    }

    pub(crate) fn reset(&mut self) {
        self.clear_window();
        self.last_amplitude = 0.0;
        self.previous_power = 0.0;
        self.previous_valid = true;
        self.target = 1.0;
        self.factor = 1.0;
        self.selected = false;
        self.counters = OnsetCounters::default();
        self.last_features = None;
        self.probability = None;
        self.applied = 1.0;
    }

    pub(crate) fn step(&mut self, amplitude: f32, fast: f32, slow: f32) -> f32 {
        let a = f64::from(amplitude);
        self.counters.frames = self.counters.frames.saturating_add(1);
        self.power += a * a;
        self.amplitude += a;
        self.difference += (a - self.last_amplitude).abs();
        self.peak = self.peak.max(a);
        self.last_amplitude = a;
        self.phase += 1;
        if a > 1.0 {
            self.invalid = true;
            self.selected = false;
            self.target = 1.0;
            self.probability = None;
        }
        if self.phase == self.hop {
            self.counters.hops += 1;
            let power = self.power / self.hop as f64;
            self.last_features = if self.invalid || !self.previous_valid {
                None
            } else {
                features(
                    power,
                    self.peak,
                    self.previous_power,
                    self.difference / self.hop as f64,
                    self.amplitude / self.hop as f64,
                    f64::from(fast),
                    f64::from(slow),
                )
            };
            self.previous_power = power;
            self.previous_valid = !self.invalid;
            self.selected = false;
            self.target = 1.0;
            self.probability = None;
            if ![44100.0, 48000.0, 96000.0].contains(&self.rate) || self.model.is_none() {
                self.counters.unavailable += 1;
            } else if let Some(p) = self
                .last_features
                .and_then(|x| self.model.as_ref().and_then(|m| m.predict(&x)))
            {
                self.probability = Some(p);
                if p >= 0.9 {
                    self.target = 1.0 + 0.25 * ((p - 0.9) / 0.1).clamp(0.0, 1.0);
                    self.selected = true;
                    self.counters.onset += 1;
                } else if p <= 0.1 {
                    self.target = 1.0 - 0.25 * ((0.1 - p) / 0.1).clamp(0.0, 1.0);
                    self.selected = true;
                    self.counters.nononset += 1;
                } else {
                    self.counters.uncertain += 1;
                }
            } else {
                self.counters.invalid_or_silent += 1;
            }
            self.clear_window();
        }
        self.factor =
            (self.target + self.coefficient * (self.factor - self.target)).clamp(0.75, 1.25);
        self.applied = if self.selected { self.factor } else { 1.0 };
        self.applied
    }

    pub(crate) fn snapshot(&self) -> OnsetSnapshot {
        OnsetSnapshot {
            counters: self.counters,
            hop_frames: self.hop,
            features: self.last_features,
            probability: self.probability,
            attack_multiplier: self.applied,
        }
    }
}
