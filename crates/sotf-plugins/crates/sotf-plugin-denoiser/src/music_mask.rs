//! Explicit QA-only musical gain selection without classical-estimator feedback.
//!
//! Learned gains are read only at synthesis. Native MCRA, decision-directed
//! history and optional classical gain paths continue independently. No factory
//! selector, speech-pretrained asset or production admission is introduced.

// Rust guideline compliant 2026-02-21
use crate::DenoiserPlugin;
use rustfft::num_complex::Complex;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

/// Fixed feature width bounds model work.
pub const FEATURES: usize = 6;
/// Fixed hidden width bounds model work.
pub const HIDDEN: usize = 8;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModel {
    format: String,
    features: usize,
    hidden: usize,
    weights: [[f64; HIDDEN]; FEATURES],
    bias: [f64; HIDDEN],
    output_weights: [f64; HIDDEN],
    output_bias: f64,
    policy: Value,
    provenance: Value,
    deployment_admitted: bool,
}

/// Unadmitted generated-music model with fixed storage and explicit provenance.
#[derive(Debug)]
pub struct MusicMask {
    weights: [[f64; HIDDEN]; FEATURES],
    bias: [f64; HIDDEN],
    output_weights: [f64; HIDDEN],
    output_bias: f64,
}

impl MusicMask {
    /// Validate bounded JSON, explicit SHA256 and the exact generated-music policy.
    ///
    /// # Errors
    /// Rejects incorrect size/hash, dimensions, provenance, policy or coefficients.
    pub fn from_json(bytes: &[u8], sha: &str) -> Result<Self, String> {
        if bytes.is_empty()
            || bytes.len() > 65536
            || sha.len() != 64
            || !Sha256::digest(bytes)
                .iter()
                .zip(sha.as_bytes().as_chunks::<2>().0)
                .all(|(&v, pair)| {
                    let hex = b"0123456789abcdef";
                    pair[0] == hex[usize::from(v >> 4)] && pair[1] == hex[usize::from(v & 15)]
                })
        {
            return Err("music-mask size/hash mismatch".into());
        }
        let raw: RawModel = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        let policy = json!({"sample_rates":[44100,48000,96000],"fft_sizes":[512,2048],"maximum_channels":2,
            "feature_history_ms":30.0,"gain_attack_ms":5.0,"gain_release_ms":30.0,"reference_reduction_db":20.0,
            "maximum_added_lookahead_samples":0,"fallback":"independent_native_denoiser","drain":"freeze_last_learned_gains"});
        if raw.format != "sotf-music-noise-mask-v1"
            || raw.features != FEATURES
            || raw.hidden != HIDDEN
            || raw.deployment_admitted
            || raw.policy != policy
            || raw.provenance
                != json!({"origin":"project-generated polyphonic and percussive music/noise mixtures",
                "license":"GPL-3.0-or-later","external_training_audio":false,"speech_pretrained":false})
            || raw
                .weights
                .iter()
                .flatten()
                .chain(&raw.bias)
                .chain(&raw.output_weights)
                .chain(std::iter::once(&raw.output_bias))
                .any(|v| !v.is_finite() || v.abs() > 16.0)
        {
            return Err("unsupported unadmitted music-mask contract".into());
        }
        Ok(Self {
            weights: raw.weights,
            bias: raw.bias,
            output_weights: raw.output_weights,
            output_bias: raw.output_bias,
        })
    }

    /// Infer a bounded gain ratio; invalid normalized evidence yields no selection.
    pub fn predict(&self, features: &[f64; FEATURES]) -> Option<f32> {
        if features
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
        {
            return None;
        }
        let mut logit = self.output_bias;
        for (j, (&bias, &output)) in self.bias.iter().zip(&self.output_weights).enumerate() {
            let z = bias
                + features
                    .iter()
                    .enumerate()
                    .map(|(k, v)| (2.0 * v - 1.0) * self.weights[k][j])
                    .sum::<f64>();
            logit += z.tanh() * output;
        }
        let result = if logit >= 0.0 {
            1.0 / (1.0 + (-logit).exp())
        } else {
            let e = logit.exp();
            e / (1.0 + e)
        };
        result.is_finite().then_some(result as f32)
    }
}

/// Extract one causal musical feature row using replicate spectral boundaries.
///
/// Returns `None` for malformed, negative or nonfinite/overloaded power evidence.
pub fn spectral_features(
    power: &[f64],
    previous: &[f64],
    bin: usize,
    fft_size: usize,
) -> Option<[f64; FEATURES]> {
    let bins = fft_size / 2 + 1;
    if ![512, 2048].contains(&fft_size)
        || power.len() != bins
        || previous.len() != bins
        || bin >= bins
    {
        return None;
    }
    if power
        .iter()
        .chain(previous)
        .any(|v| !v.is_finite() || *v < 0.0 || *v > 1e12)
    {
        return None;
    }
    let mean = power.iter().sum::<f64>() / bins as f64;
    Some(features_validated(power, previous, bin, fft_size, mean))
}

fn features_validated(
    power: &[f64],
    previous: &[f64],
    bin: usize,
    fft_size: usize,
    mean: f64,
) -> [f64; FEATURES] {
    let epsilon = 1e-20;
    let at =
        |offset: isize| power[bin.saturating_add_signed(offset).min(power.len() - 1)].max(epsilon);
    let local = (-2..=2).map(at).sum::<f64>() / 5.0;
    let arithmetic = (-4..=4).map(at).sum::<f64>() / 9.0;
    let geometric = ((-4..=4).map(|j| at(j).ln()).sum::<f64>() / 9.0).exp();
    let current = power[bin].max(epsilon);
    [
        ((current / local.max(epsilon)).ln() + 4.0) / 8.0,
        ((current / mean.max(epsilon)).ln() + 8.0) / 16.0,
        geometric / arithmetic.max(epsilon),
        ((current / previous[bin].max(epsilon)).ln() + 4.0) / 8.0,
        bin as f64 / (power.len() - 1) as f64,
        (10.0 * (mean.max(epsilon) / (fft_size as f64 / 2.0)).log10() + 100.0) / 100.0,
    ]
    .map(|v| v.clamp(0.0, 1.0))
}

/// QA selection decisions for complete source analysis frames.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct MusicCounters {
    /// Source FFT frames observed, including the defined half-window startup prefix.
    pub frames: u64,
    /// Emission bins selecting the learned gain.
    pub neural_bins: u64,
    /// Valid learned bins prepared while selection is disabled.
    pub disabled_bins: u64,
    /// Bins retaining native gains because model/context is unavailable.
    pub unavailable_bins: u64,
    /// Bins retaining native gains after invalid evidence.
    pub invalid_bins: u64,
    /// DC/Nyquist bins deliberately retaining native gains.
    pub boundary_bins: u64,
}

#[derive(Debug)]
pub(crate) struct GainContext<'a> {
    pub sample_rate: f64,
    pub fft_size: usize,
    pub reduction_db: f32,
    pub floor: f32,
    pub transparency: f32,
    pub curve: &'a [f32],
}

/// Prepared QA gain history, completely separate from native classical state.
#[derive(Debug)]
pub struct MusicPath {
    model: Option<MusicMask>,
    enabled: bool,
    previous: Vec<Vec<f64>>,
    gains: Vec<Vec<f32>>,
    valid: Vec<Vec<bool>>,
    initialized: Vec<bool>,
    powers: Vec<f64>,
    last_positive_depth: f32,
    counters: MusicCounters,
}

impl MusicPath {
    fn new(model: Option<MusicMask>, channels: usize, bins: usize) -> Self {
        Self {
            model,
            enabled: false,
            previous: vec![vec![0.0; bins]; channels],
            gains: vec![vec![1.0; bins]; channels],
            valid: vec![vec![false; bins]; channels],
            initialized: vec![false; channels],
            powers: vec![0.0; bins],
            last_positive_depth: 20.0,
            counters: MusicCounters::default(),
        }
    }

    pub(crate) fn reset(&mut self) {
        for v in &mut self.previous {
            v.fill(0.0);
        }
        for v in &mut self.gains {
            v.fill(1.0);
        }
        for v in &mut self.valid {
            v.fill(false);
        }
        self.initialized.fill(false);
        self.powers.fill(0.0);
        self.last_positive_depth = 20.0;
        self.counters = MusicCounters::default();
    }

    pub(crate) fn prepare(&mut self, spectra: &[Vec<Complex<f32>>], context: GainContext<'_>) {
        self.counters.frames = self.counters.frames.saturating_add(1);
        let bins = self.powers.len();
        let supported = [44100.0, 48000.0, 96000.0].contains(&context.sample_rate)
            && [512, 2048].contains(&context.fft_size)
            && spectra.len() <= 2
            && context.curve.len() == bins;
        if !supported || self.model.is_none() {
            for v in &mut self.valid {
                v.fill(false);
            }
            self.counters.unavailable_bins = self
                .counters
                .unavailable_bins
                .saturating_add((bins * spectra.len()) as u64);
            return;
        }
        let model = self.model.as_ref().expect("checked prepared model");
        let hop = context.fft_size as f64 / 2.0;
        let retention = (-hop / context.sample_rate / 0.03).exp();
        let attack = (-hop / context.sample_rate / 0.005).exp() as f32;
        let release = (-hop / context.sample_rate / 0.03).exp() as f32;
        if context.reduction_db > 0.0 {
            self.last_positive_depth = context.reduction_db;
        }
        for (channel, spectrum) in spectra.iter().enumerate() {
            let mut finite = spectrum.len() == bins;
            for (dst, v) in self.powers.iter_mut().zip(spectrum) {
                let re = f64::from(v.re);
                let im = f64::from(v.im);
                *dst = re * re + im * im;
                finite &= dst.is_finite() && *dst <= 1e12;
            }
            if !finite {
                self.valid[channel].fill(false);
                self.initialized[channel] = false;
                self.counters.invalid_bins = self.counters.invalid_bins.saturating_add(bins as u64);
                continue;
            }
            if !self.initialized[channel] {
                self.previous[channel].copy_from_slice(&self.powers);
                self.initialized[channel] = true;
            }
            let mean = self.powers.iter().sum::<f64>() / bins as f64;
            for bin in 0..bins {
                if bin == 0 || bin == bins - 1 {
                    self.valid[channel][bin] = false;
                    self.counters.boundary_bins = self.counters.boundary_bins.saturating_add(1);
                    continue;
                }
                let features = features_validated(
                    &self.powers,
                    &self.previous[channel],
                    bin,
                    context.fft_size,
                    mean,
                );
                if let Some(prediction) = model.predict(&features) {
                    let shaped = prediction
                        .powf(self.last_positive_depth / 20.0)
                        .max(context.floor);
                    let shaped = 1.0 + context.curve[bin] * (shaped - 1.0);
                    let target = shaped + context.transparency * (1.0 - shaped);
                    let old = self.gains[channel][bin];
                    let coefficient = if target > old { attack } else { release };
                    self.gains[channel][bin] =
                        (target + coefficient * (old - target)).clamp(context.floor, 1.0);
                    self.valid[channel][bin] = true;
                    if self.enabled {
                        self.counters.neural_bins = self.counters.neural_bins.saturating_add(1);
                    } else {
                        self.counters.disabled_bins = self.counters.disabled_bins.saturating_add(1);
                    }
                } else {
                    self.valid[channel][bin] = false;
                    self.counters.invalid_bins = self.counters.invalid_bins.saturating_add(1);
                }
            }
            for (old, &power) in self.previous[channel].iter_mut().zip(&self.powers) {
                *old = retention * (*old) + (1.0 - retention) * power;
            }
        }
    }

    pub(crate) fn gain(&self, channel: usize, bin: usize, native: f32, unity_mix: f32) -> f32 {
        if !self.enabled || !self.valid[channel][bin] {
            return native;
        }
        if unity_mix == 1.0 {
            return 1.0;
        }
        let gain = self.gains[channel][bin];
        if unity_mix == 0.0 {
            gain
        } else {
            gain + unity_mix * (1.0 - gain)
        }
    }
}

impl DenoiserPlugin {
    /// Prepare an explicit QA mode on a fresh stream, initially disabled.
    ///
    /// # Errors
    /// Rejects replacement after source input or drain; reset first. Allocation
    /// and model loading belong on the control thread. No factory selector exists.
    pub fn install_qa_music_mask(&mut self, model: Option<MusicMask>) -> Result<(), String> {
        if self.io.has_input || self.io.drain_remaining.is_some() {
            return Err("install the music mask on a fresh denoiser stream".into());
        }
        self.qa_music_mask = Some(MusicPath::new(
            model,
            self.config.channels,
            self.config.spectrum_size,
        ));
        Ok(())
    }

    /// Toggle the prepared QA synthesis selection without changing classical state.
    ///
    /// # Errors
    /// Rejects absent QA state or selection changes after drain has begun.
    pub fn set_qa_music_mask_enabled(&mut self, enabled: bool) -> Result<(), String> {
        if self.io.drain_remaining.is_some() {
            return Err("reset before changing music-mask selection after drain".into());
        }
        self.qa_music_mask
            .as_mut()
            .ok_or("prepare a QA music mask before selection")?
            .enabled = enabled;
        Ok(())
    }

    /// Copy QA decision counters without allocating.
    pub fn qa_music_mask_counters(&self) -> Option<MusicCounters> {
        self.qa_music_mask.as_ref().map(|p| p.counters)
    }
}
