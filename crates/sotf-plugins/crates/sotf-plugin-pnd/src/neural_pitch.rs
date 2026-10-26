//! Optional causal CREPE analysis; audio rendering retains its classical fallback.
//!
//! A fixed-size source window is sent every 10 ms to a bounded latest-frame
//! worker. Resampling, normalization, ONNX execution, and decoding happen there.
//! Results carry the source end clock; age includes the entire 64 ms window.
//! No audio is buffered for the worker, so synthesis latency is unchanged.
//! Confidence is a model activation, not a calibrated probability of correctness.

use crate::NeuralPitchConfig;
use plugins_inference::{AsyncInference, InferenceModel};
use sha2::{Digest, Sha256};
use std::f64::consts::PI;
use tract_onnx::prelude::*;

const MODEL_SAMPLES: usize = 1024;
const MODEL_RATE: f64 = 16_000.0;
const MAX_SOURCE_SAMPLES: usize = 12_288;
const PITCH_BINS: usize = 360;
type Runnable = TypedRunnableModel<TypedModel>;

enum ActivationModel {
    Tract(Box<Runnable>),
    #[cfg(feature = "neural-pitch-ort")]
    Ort(Box<crate::ort_pitch::OrtPitchModel>),
}

struct SourceFrame {
    samples: [f32; MAX_SOURCE_SAMPLES],
    end_sample: u64,
}

/// A decoded observation tied to the source clock.
#[derive(Debug, Clone, Copy)]
pub struct PitchObservation {
    /// Estimated fundamental in hertz; zero denotes a silent frame.
    pub frequency_hz: f32,
    /// Maximum model activation, before application policy gates.
    pub confidence: f32,
    /// Exclusive end of the observed source window.
    pub end_sample: u64,
}

/// One policy-accepted result copied during a single nonblocking worker read.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AcceptedPitch {
    pub ratio: f32,
    pub observation: PitchObservation,
    #[cfg(feature = "qa")]
    pub source_clock: u64,
    #[cfg(feature = "qa")]
    pub window_samples: u64,
}

/// Fixed-shape pretrained pitch model, prepared exclusively on a control thread.
pub struct CrepeModel {
    model: ActivationModel,
    resampling: Vec<Vec<(usize, f64)>>,
    source_len: usize,
}

impl std::fmt::Debug for CrepeModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CrepeModel")
            .field("backend", &self.backend_name())
            .field("source_len", &self.source_len)
            .finish_non_exhaustive()
    }
}

impl CrepeModel {
    /// Identify the selected execution path without exposing foreign runtime handles.
    pub fn backend_name(&self) -> &'static str {
        match &self.model {
            ActivationModel::Tract(_) => "tract",
            #[cfg(feature = "neural-pitch-ort")]
            ActivationModel::Ort(model) => model.backend_name(),
        }
    }
    /// Load and verify an exported CREPE model at the actual source rate.
    ///
    /// # Errors
    /// Rejects invalid rates, mismatched hashes, unsupported metadata or shapes,
    /// unreadable files, and models that cannot execute the fixed contract.
    pub fn load(config: &NeuralPitchConfig, sample_rate: f64) -> Result<Self, String> {
        if !sample_rate.is_finite() || !(16_000.0..=192_000.0).contains(&sample_rate) {
            return Err("Neural pitch supports source rates from 16 to 192 kHz".into());
        }
        let bytes = std::fs::read(&config.model_path).map_err(|e| e.to_string())?;
        let actual: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if config.sha256.len() != 64 || config.sha256 != actual {
            return Err("Neural pitch model SHA-256 mismatch".into());
        }
        let onnx = tract_onnx::onnx();
        let proto = onnx
            .proto_model_for_read(&mut bytes.as_slice())
            .map_err(|e| e.to_string())?;
        for (key, expected) in [
            ("sotf_contract", "crepe-tiny-v1"),
            ("sample_rate", "16000"),
            ("frame_samples", "1024"),
            ("normalization", "zero-mean-sample-std-ddof1-min1e-10"),
        ] {
            if !proto
                .metadata_props
                .iter()
                .any(|p| p.key == key && p.value == expected)
            {
                return Err(format!("Neural pitch model metadata mismatch: {key}"));
            }
        }
        let model = onnx
            .model_for_proto_model(&proto)
            .map_err(|e| e.to_string())?
            .into_optimized()
            .map_err(|e| e.to_string())?
            .into_runnable()
            .map_err(|e| e.to_string())?;
        if model
            .model()
            .input_outlets()
            .map_err(|e| e.to_string())?
            .len()
            != 1
            || model
                .model()
                .output_outlets()
                .map_err(|e| e.to_string())?
                .len()
                != 1
        {
            return Err("CREPE requires exactly one input and output".into());
        }
        for (fact, shape) in [
            (
                model.model().input_fact(0).map_err(|e| e.to_string())?,
                &[1, MODEL_SAMPLES][..],
            ),
            (
                model.model().output_fact(0).map_err(|e| e.to_string())?,
                &[1, PITCH_BINS][..],
            ),
        ] {
            if fact.datum_type != f32::datum_type() || fact.shape.as_concrete() != Some(shape) {
                return Err("CREPE requires f32 [1,1024] -> [1,360]".into());
            }
        }
        let mut result = Self {
            model: ActivationModel::Tract(Box::new(model)),
            resampling: resampling_weights(sample_rate),
            source_len: (MODEL_SAMPLES as f64 * sample_rate / MODEL_RATE).ceil() as usize,
        };
        result.activations(&[0.0; MODEL_SAMPLES])?;
        Ok(result)
    }

    /// Prepare a QA alternative using the identical source preprocessing and decoder.
    ///
    /// # Errors
    /// Rejects unsupported rates, model/library identity, session or metadata failures.
    #[cfg(feature = "neural-pitch-ort")]
    pub fn load_ort(
        config: &NeuralPitchConfig,
        sample_rate: f64,
        runtime: &crate::ort_pitch::OrtPitchConfig,
    ) -> Result<Self, String> {
        if !sample_rate.is_finite() || !(16_000.0..=192_000.0).contains(&sample_rate) {
            return Err("Neural pitch supports source rates from16 to192kHz".into());
        }
        let model = crate::ort_pitch::OrtPitchModel::load(config, runtime)?;
        let mut result = Self {
            model: ActivationModel::Ort(Box::new(model)),
            resampling: resampling_weights(sample_rate),
            source_len: (MODEL_SAMPLES as f64 * sample_rate / MODEL_RATE).ceil() as usize,
        };
        result.activations(&[0.0; MODEL_SAMPLES])?;
        Ok(result)
    }

    /// Analyze one complete source-rate window off the audio thread.
    ///
    /// The required window is `ceil(1024 * source_rate / 16000)` samples.
    /// This is also the reference path for offline quality and timing evaluation.
    ///
    /// # Errors
    /// Rejects a wrong window length, non-finite samples, or model failures.
    pub fn analyze_window(
        &mut self,
        samples: &[f32],
        end_sample: u64,
    ) -> Result<PitchObservation, String> {
        if samples.len() != self.source_len || samples.iter().any(|x| !x.is_finite()) {
            return Err("CREPE source window has invalid length or non-finite samples".into());
        }
        let mut frame = [0.0_f32; MODEL_SAMPLES];
        for (value, weights) in frame.iter_mut().zip(&self.resampling) {
            *value = weights
                .iter()
                .map(|&(i, w)| f64::from(samples[i]) * w)
                .sum::<f64>() as f32;
        }
        let mean = frame.iter().map(|&x| f64::from(x)).sum::<f64>() / MODEL_SAMPLES as f64;
        let energy = frame
            .iter()
            .map(|&x| (f64::from(x) - mean).powi(2))
            .sum::<f64>();
        let mut observation = PitchObservation {
            frequency_hz: 0.0,
            confidence: 0.0,
            end_sample,
        };
        if energy / MODEL_SAMPLES as f64 <= 1e-6 {
            return Ok(observation);
        }
        let std = (energy / (MODEL_SAMPLES - 1) as f64).sqrt().max(1e-10);
        for x in &mut frame {
            *x = ((f64::from(*x) - mean) / std) as f32;
        }
        let activation = self.activations(&frame)?;
        let (hz, confidence) = decode(&activation);
        observation.frequency_hz = hz;
        observation.confidence = confidence;
        Ok(observation)
    }

    /// Evaluate a normalized 16 kHz frame for cross-runtime parity and benchmarks.
    ///
    /// This performs inference and allocates; call only off the audio thread.
    ///
    /// # Errors
    /// Rejects non-finite input, runtime failures, and invalid model activations.
    pub fn activations(
        &mut self,
        frame: &[f32; MODEL_SAMPLES],
    ) -> Result<[f32; PITCH_BINS], String> {
        if frame.iter().any(|v| !v.is_finite()) {
            return Err("Non-finite CREPE input".into());
        }
        let model = match &mut self.model {
            ActivationModel::Tract(model) => model,
            #[cfg(feature = "neural-pitch-ort")]
            ActivationModel::Ort(model) => return model.activations(frame),
        };
        let input = Tensor::from_shape(&[1, MODEL_SAMPLES], frame).map_err(|e| e.to_string())?;
        let output = model.run(tvec!(input.into())).map_err(|e| e.to_string())?;
        let view = output[0]
            .to_array_view::<f32>()
            .map_err(|e| e.to_string())?;
        if view.shape() != [1, PITCH_BINS] {
            return Err("Invalid CREPE output shape".into());
        }
        let mut result = [0.0; PITCH_BINS];
        for (dst, &src) in result.iter_mut().zip(view.iter()) {
            if !src.is_finite() || !(0.0..=1.0).contains(&src) {
                return Err("Invalid CREPE activation".into());
            }
            *dst = src;
        }
        Ok(result)
    }
}

struct WorkerModel(CrepeModel);

impl InferenceModel for WorkerModel {
    type Input = SourceFrame;
    type Output = PitchObservation;

    fn run(&mut self, input: &SourceFrame) -> Result<PitchObservation, String> {
        self.0
            .analyze_window(&input.samples[..self.0.source_len], input.end_sample)
    }
}

fn decode(activation: &[f32; PITCH_BINS]) -> (f32, f32) {
    let peak = (0..PITCH_BINS)
        .max_by(|&a, &b| activation[a].total_cmp(&activation[b]))
        .unwrap_or(0);
    let mut weight = 0.0_f64;
    let mut cents = 0.0_f64;
    for (i, &a) in activation
        .iter()
        .enumerate()
        .take((peak + 5).min(PITCH_BINS))
        .skip(peak.saturating_sub(4))
    {
        weight += f64::from(a);
        cents += f64::from(a) * (1_997.379_408_437_619 + 20.0 * i as f64);
    }
    if weight <= 0.0 {
        return (0.0, 0.0);
    }
    (
        (10.0 * 2.0_f64.powf(cents / weight / 1200.0)) as f32,
        activation[peak],
    )
}

fn resampling_weights(rate: f64) -> Vec<Vec<(usize, f64)>> {
    let source_len = (MODEL_SAMPLES as f64 * rate / MODEL_RATE).ceil() as usize;
    let cutoff = MODEL_RATE / rate;
    let radius = (32.0 / cutoff).ceil() as isize;
    (0..MODEL_SAMPLES)
        .map(|j| {
            let position = j as f64 / cutoff;
            let center = position.floor() as isize;
            let mut weights = Vec::new();
            for i in (center - radius)..=(center + radius) {
                if i < 0 || i as usize >= source_len {
                    continue;
                }
                let distance = i as f64 - position;
                let phase = PI * distance * cutoff;
                let sinc = if phase.abs() < 1e-12 {
                    1.0
                } else {
                    phase.sin() / phase
                };
                let window = 0.42
                    + 0.5 * (PI * distance / radius as f64).cos()
                    + 0.08 * (2.0 * PI * distance / radius as f64).cos();
                weights.push((i as usize, sinc * window * cutoff));
            }
            let sum: f64 = weights.iter().map(|&(_, w)| w).sum();
            for (_, w) in &mut weights {
                *w /= sum;
            }
            weights
        })
        .collect()
}

/// Audio-side bounded frame producer and confidence-gated result reader.
pub struct NeuralPitchAssistant {
    worker: AsyncInference<WorkerModel>,
    config: NeuralPitchConfig,
    ring: Vec<f32>,
    write: usize,
    clock: u64,
    hop: usize,
    until_hop: usize,
    max_age: u64,
    backend_name: &'static str,
}

impl std::fmt::Debug for NeuralPitchAssistant {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NeuralPitchAssistant")
            .field("source_window", &self.ring.len())
            .field("source_clock", &self.clock)
            .field("hop", &self.hop)
            .finish_non_exhaustive()
    }
}

impl NeuralPitchAssistant {
    /// Prepare a model and fixed-size buffers before audio starts.
    ///
    /// # Errors
    /// Rejects invalid policy, unavailable models, unsupported rates, or spawn errors.
    pub fn new(config: NeuralPitchConfig, sample_rate: f64) -> Result<Self, String> {
        Self::validate_policy(&config)?;
        let model = CrepeModel::load(&config, sample_rate)?;
        Self::from_model(config, sample_rate, model)
    }

    /// Prepare a QA alternative on the same bounded worker/framing/policy path.
    ///
    /// # Errors
    /// Rejects invalid policy or model/library/session initialization failures.
    #[cfg(feature = "neural-pitch-ort")]
    pub fn new_ort(
        config: NeuralPitchConfig,
        sample_rate: f64,
        runtime: &crate::ort_pitch::OrtPitchConfig,
    ) -> Result<Self, String> {
        Self::validate_policy(&config)?;
        let model = CrepeModel::load_ort(&config, sample_rate, runtime)?;
        Self::from_model(config, sample_rate, model)
    }

    fn validate_policy(config: &NeuralPitchConfig) -> Result<(), String> {
        if !config.min_confidence.is_finite()
            || !(0.0..=1.0).contains(&config.min_confidence)
            || config.min_confidence == 0.0
            || !config.max_age_ms.is_finite()
            || !(64.0..=1000.0).contains(&config.max_age_ms)
            || !config.max_deviation_cents.is_finite()
            || !(0.0..=100.0).contains(&config.max_deviation_cents)
        {
            return Err("Invalid neural pitch confidence, age, or drift policy".into());
        }
        Ok(())
    }

    fn from_model(
        config: NeuralPitchConfig,
        sample_rate: f64,
        model: CrepeModel,
    ) -> Result<Self, String> {
        let window = (MODEL_SAMPLES as f64 * sample_rate / MODEL_RATE).ceil() as usize;
        let hop = (sample_rate / 100.0).round() as usize;
        let max_age = (sample_rate * 0.001 * f64::from(config.max_age_ms)) as u64;
        if max_age < window as u64 {
            return Err("Neural pitch age limit is shorter than one source window".into());
        }
        let backend_name = model.backend_name();
        Ok(Self {
            worker: AsyncInference::spawn(WorkerModel(model), 2, "pnd-crepe")?,
            config,
            ring: vec![0.0; window],
            write: 0,
            clock: 0,
            hop,
            until_hop: window,
            max_age,
            backend_name,
        })
    }

    /// Identify the prepared worker model without querying the foreign runtime.
    pub fn backend_name(&self) -> &'static str {
        self.backend_name
    }

    /// Return the explicitly configured source channel.
    pub fn source_channel(&self) -> usize {
        self.config.source_channel
    }

    /// Advance the source clock and enqueue a complete causal window without waiting.
    pub fn push(&mut self, sample: f32) {
        self.ring[self.write] = sample;
        self.write = (self.write + 1) % self.ring.len();
        self.clock = self.clock.saturating_add(1);
        self.until_hop -= 1;
        if self.until_hop == 0 {
            self.until_hop = self.hop;
            let mut frame = SourceFrame {
                samples: [0.0; MAX_SOURCE_SAMPLES],
                end_sample: self.clock,
            };
            let first = self.ring.len() - self.write;
            frame.samples[..first].copy_from_slice(&self.ring[self.write..]);
            frame.samples[first..self.ring.len()].copy_from_slice(&self.ring[..self.write]);
            let _ = self.worker.try_send(frame);
        }
    }

    /// Read a fresh, supported drift observation; otherwise retain classical analysis.
    pub fn estimate_against_reference(&self, reference_hz: f32) -> Option<(f32, f32)> {
        self.accepted_estimate(reference_hz)
            .map(|p| (p.ratio, p.observation.confidence))
    }

    pub(crate) fn accepted_estimate(&self, reference_hz: f32) -> Option<AcceptedPitch> {
        self.worker
            .with_latest(|p| {
                accepted_ratio(
                    *p,
                    self.clock,
                    self.ring.len() as u64,
                    self.max_age,
                    reference_hz,
                    &self.config,
                )
                .map(|(ratio, _)| AcceptedPitch {
                    ratio,
                    observation: *p,
                    #[cfg(feature = "qa")]
                    source_clock: self.clock,
                    #[cfg(feature = "qa")]
                    window_samples: self.ring.len() as u64,
                })
            })
            .flatten()
    }

    /// Invalidate all previous-source results without dropping or joining the worker.
    pub fn reset(&mut self) {
        self.worker.reset();
        self.ring.fill(0.0);
        self.write = 0;
        self.clock = 0;
        self.until_hop = self.ring.len();
    }
}

fn accepted_ratio(
    p: PitchObservation,
    clock: u64,
    window: u64,
    max_age: u64,
    reference: f32,
    config: &NeuralPitchConfig,
) -> Option<(f32, f32)> {
    let age = clock.checked_sub(p.end_sample)?.checked_add(window)?;
    if age > max_age
        || !p.confidence.is_finite()
        || p.confidence < config.min_confidence
        || !p.frequency_hz.is_finite()
        || !(32.7..=2006.0).contains(&p.frequency_hz)
        || !reference.is_finite()
        || !(32.7..=2006.0).contains(&reference)
    {
        return None;
    }
    let ratio = p.frequency_hz / reference;
    let cents = 1200.0 * ratio.log2();
    (cents.abs() <= config.max_deviation_cents).then_some((ratio, p.confidence))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn config() -> NeuralPitchConfig {
        NeuralPitchConfig {
            model_path: String::new(),
            sha256: String::new(),
            min_confidence: 0.8,
            max_age_ms: 150.0,
            max_deviation_cents: 100.0,
            source_channel: 0,
        }
    }

    #[test]
    fn policy_rejects_stale_silent_unvoiced_octave_and_future_results() {
        let p = PitchObservation {
            frequency_hz: 441.0,
            confidence: 0.95,
            end_sample: 4000,
        };
        assert!(accepted_ratio(p, 5000, 3072, 7200, 440.0, &config()).is_some());
        for bad in [
            PitchObservation {
                end_sample: 6000,
                ..p
            },
            PitchObservation { end_sample: 0, ..p },
            PitchObservation {
                frequency_hz: 0.0,
                ..p
            },
            PitchObservation {
                frequency_hz: 880.0,
                ..p
            },
            PitchObservation {
                confidence: 0.2,
                ..p
            },
            PitchObservation {
                frequency_hz: f32::NAN,
                ..p
            },
        ] {
            assert!(accepted_ratio(bad, 5000, 3072, 7200, 440.0, &config()).is_none());
        }
        assert!(accepted_ratio(p, 5000, 3072, 7200, 0.0, &config()).is_none());
    }

    #[test]
    fn decoder_uses_local_weighted_cents_without_octave_folding() {
        let mut a = [0.0; PITCH_BINS];
        a[100] = 0.8;
        a[101] = 0.2;
        let (hz, confidence) = decode(&a);
        let expected = 10.0 * 2.0_f64.powf((1_997.379_408_437_619 + 2004.0) / 1200.0);
        assert!((f64::from(hz) - expected).abs() < 1e-4);
        assert_eq!(confidence, 0.8);
    }

    #[test]
    fn resampling_preserves_dc_and_rejects_above_model_nyquist() {
        for rate in [16_000.0, 44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let weights = resampling_weights(rate);
            for row in &weights {
                assert!((row.iter().map(|&(_, w)| w).sum::<f64>() - 1.0).abs() < 1e-12);
            }
            if rate > 20_000.0 {
                let power = weights[100..924]
                    .iter()
                    .map(|row| {
                        row.iter()
                            .map(|&(i, w)| w * (2.0 * PI * 10_000.0 * i as f64 / rate).sin())
                            .sum::<f64>()
                            .powi(2)
                    })
                    .sum::<f64>()
                    / 824.0;
                assert!(power < 1e-6, "rate {rate}: alias power {power}");
            }
        }
    }
}
