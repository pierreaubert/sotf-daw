//! Causal, bounded RNNoise speech observations; model work never runs on audio.
//!
//! Each window starts from reset model state and replays 100 ms of source.
//! Dropped windows therefore cannot corrupt a recurrent stream. This trades
//! worker compute for independent observations and exact reset/source identity.

use crate::speech_protection::{SpeechProtectionConfig, SpeechProtectionData};
use plugins_denoiser::rnnoise::RnnoiseBackend;
use plugins_inference::{AsyncInference, InferenceModel};
use std::f64::consts::PI;

const MODEL_RATE: f64 = 48_000.0;
const WINDOW_SAMPLES: usize = 4800;
const MAX_SOURCE_SAMPLES: usize = 19_200;

struct SourceWindow {
    samples: [f32; MAX_SOURCE_SAMPLES],
    end_sample: u64,
}

#[derive(Debug, Clone, Copy)]
pub(super) struct Observation {
    pub(super) score: f32,
    pub(super) end_sample: u64,
}

struct Model {
    backend: RnnoiseBackend,
    weights: Vec<Vec<(usize, f64)>>,
}

impl InferenceModel for Model {
    type Input = SourceWindow;
    type Output = Observation;

    fn run(&mut self, input: &SourceWindow) -> Result<Observation, String> {
        self.backend.reset();
        let mut frame = [0.0; 480];
        for hop in 0..10 {
            for (index, value) in frame.iter_mut().enumerate() {
                let sum = self.weights[hop * 480 + index]
                    .iter()
                    .map(|&(source, weight)| f64::from(input.samples[source]) * weight)
                    .sum::<f64>();
                *value = (sum as f32).clamp(-1.0, 1.0);
            }
            if self.backend.process(&mut frame, 480, 1, true) != 480 {
                return Err("RNNoise speech analysis did not consume a complete frame".into());
            }
        }
        let score = self.backend.analyzer_data().vad_probability;
        if !score.is_finite() || !(0.0..=1.0).contains(&score) {
            return Err("Invalid RNNoise speech observation".into());
        }
        Ok(Observation {
            score,
            end_sample: input.end_sample,
        })
    }
}

fn resampling_weights(rate: f64) -> Vec<Vec<(usize, f64)>> {
    let source_len = (rate * 0.1).ceil() as usize;
    let ratio = rate / MODEL_RATE;
    let cutoff = (MODEL_RATE / rate).min(1.0);
    // Sixteen sinc lobes with a Blackman window; all support is clipped to
    // already observed samples, with boundary DC normalization.
    let radius = (16.0 / cutoff).ceil() as isize;
    (0..WINDOW_SAMPLES)
        .map(|sample| {
            let position = sample as f64 * ratio;
            let center = position.floor() as isize;
            let mut weights = Vec::new();
            for index in center - radius..=center + radius {
                if index < 0 || index as usize >= source_len {
                    continue;
                }
                let distance = index as f64 - position;
                let phase = PI * distance * cutoff;
                let sinc = if phase.abs() < 1e-12 {
                    1.0
                } else {
                    phase.sin() / phase
                };
                let window = 0.42
                    + 0.5 * (PI * distance / radius as f64).cos()
                    + 0.08 * (2.0 * PI * distance / radius as f64).cos();
                weights.push((index as usize, sinc * cutoff * window));
            }
            let total: f64 = weights.iter().map(|&(_, weight)| weight).sum();
            for (_, weight) in &mut weights {
                *weight /= total;
            }
            weights
        })
        .collect()
}

pub(super) struct RnnoiseAssistant {
    worker: AsyncInference<Model>,
    config: SpeechProtectionConfig,
    ring: Vec<f32>,
    write: usize,
    clock: u64,
    hop: usize,
    until_hop: usize,
    max_age: u64,
    rate: f64,
    latest: Option<Observation>,
    submitted: u64,
    dropped: u64,
    observed: u64,
}

impl RnnoiseAssistant {
    pub(super) fn new(config: SpeechProtectionConfig, rate: f64) -> Result<Self, String> {
        config.validate()?;
        if !rate.is_finite() || !(8000.0..=192000.0).contains(&rate) {
            return Err("Gate neural speech supports source rates from 8 to 192 kHz".into());
        }
        let mut backend = RnnoiseBackend::new();
        backend.initialize(48_000, 1)?;
        let length = (rate * 0.1).ceil() as usize;
        let max_age = (rate * f64::from(config.max_age_ms) * 0.001).ceil() as u64;
        Ok(Self {
            worker: AsyncInference::spawn(
                Model {
                    backend,
                    weights: resampling_weights(rate),
                },
                2,
                "gate-speech",
            )?,
            config,
            ring: vec![0.0; length],
            write: 0,
            clock: 0,
            hop: (rate * 0.01).round().max(1.0) as usize,
            until_hop: length,
            max_age,
            rate,
            latest: None,
            submitted: 0,
            dropped: 0,
            observed: 0,
        })
    }

    pub(super) fn push(&mut self, sample: f32) {
        self.ring[self.write] = if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        self.write = (self.write + 1) % self.ring.len();
        self.clock = self.clock.saturating_add(1);
        self.until_hop -= 1;
        if self.until_hop == 0 {
            self.until_hop = self.hop;
            let mut frame = SourceWindow {
                samples: [0.0; MAX_SOURCE_SAMPLES],
                end_sample: self.clock,
            };
            let first = self.ring.len() - self.write;
            frame.samples[..first].copy_from_slice(&self.ring[self.write..]);
            frame.samples[first..self.ring.len()].copy_from_slice(&self.ring[..self.write]);
            if self.worker.try_send(frame).is_ok() {
                self.submitted += 1;
            } else {
                self.dropped += 1;
            }
            if let Some(observation) = self.worker.with_latest(|value| *value) {
                if self
                    .latest
                    .is_none_or(|previous| previous.end_sample != observation.end_sample)
                {
                    self.observed += 1;
                }
                self.latest = Some(observation);
            }
        }
    }

    pub(super) fn usable(&self) -> bool {
        self.latest.is_some_and(|value| {
            accepted(
                value,
                self.clock,
                self.ring.len() as u64,
                self.max_age,
                self.config.min_probability,
            )
        })
    }

    pub(super) fn data(&self, protection_mix: f32) -> SpeechProtectionData {
        let age = self
            .latest
            .and_then(|value| self.clock.checked_sub(value.end_sample))
            .and_then(|lag| lag.checked_add(self.ring.len() as u64));
        SpeechProtectionData {
            warmup_complete: self.latest.is_some(),
            model_frames: if self.latest.is_some() { 10 } else { 0 },
            stream_restarts: 0,
            model_score: self.latest.map(|value| value.score),
            source_age_ms: age.map(|samples| (samples as f64 * 1000.0 / self.rate) as f32),
            observation_usable: self.usable(),
            protection_mix,
            submitted_windows: self.submitted,
            dropped_windows: self.dropped,
            observed_windows: self.observed,
        }
    }

    pub(super) fn reset(&mut self) {
        self.worker.reset();
        self.ring.fill(0.0);
        self.write = 0;
        self.clock = 0;
        self.until_hop = self.ring.len();
        self.latest = None;
        self.submitted = 0;
        self.dropped = 0;
        self.observed = 0;
    }
}

pub(super) fn accepted(
    value: Observation,
    clock: u64,
    window: u64,
    max_age: u64,
    minimum: f32,
) -> bool {
    let Some(age) = clock
        .checked_sub(value.end_sample)
        .and_then(|lag| lag.checked_add(window))
    else {
        return false;
    };
    age <= max_age && value.score.is_finite() && (minimum..=1.0).contains(&value.score)
}

pub(super) enum SpeechAssistant {
    Rnnoise(Box<RnnoiseAssistant>),
    #[cfg(feature = "silero-speech")]
    Silero(Box<crate::silero_stream::SileroAssistant>),
}

impl SpeechAssistant {
    pub(super) fn new(config: SpeechProtectionConfig, rate: f64) -> Result<Self, String> {
        match &config.model {
            crate::SpeechProtectionModel::Rnnoise => {
                RnnoiseAssistant::new(config, rate).map(|x| Self::Rnnoise(Box::new(x)))
            }
            crate::SpeechProtectionModel::Silero { .. } => {
                #[cfg(feature = "silero-speech")]
                {
                    crate::silero_stream::SileroAssistant::new(config, rate)
                        .map(|x| Self::Silero(Box::new(x)))
                }
                #[cfg(not(feature = "silero-speech"))]
                {
                    Err("Silero speech requires the silero-speech build feature".into())
                }
            }
        }
    }

    pub(super) fn push(&mut self, sample: f32) {
        match self {
            Self::Rnnoise(x) => x.push(sample),
            #[cfg(feature = "silero-speech")]
            Self::Silero(x) => x.push(sample),
        }
    }

    pub(super) fn usable(&self) -> bool {
        match self {
            Self::Rnnoise(x) => x.usable(),
            #[cfg(feature = "silero-speech")]
            Self::Silero(x) => x.usable(),
        }
    }

    pub(super) fn data(&self, mix: f32) -> SpeechProtectionData {
        match self {
            Self::Rnnoise(x) => x.data(mix),
            #[cfg(feature = "silero-speech")]
            Self::Silero(x) => x.data(mix),
        }
    }

    pub(super) fn reset(&mut self) {
        match self {
            Self::Rnnoise(x) => x.reset(),
            #[cfg(feature = "silero-speech")]
            Self::Silero(x) => x.reset(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Development-set diagnostics, without per-window labels or a held-out claim.
    #[test]
    #[ignore = "offline model diagnostic requiring the workspace development fixtures"]
    fn recorded_development_observations() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap();
        let mut backend = RnnoiseBackend::new();
        backend.initialize(48_000, 1).unwrap();
        let mut model = Model {
            backend,
            weights: resampling_weights(48_000.0),
        };
        let fixtures = [
            (
                "speech-0",
                "sotf-plugin-speech-denoiser/tests/fixtures/recorded/speech-0.pcm",
            ),
            (
                "speech-1",
                "sotf-plugin-speech-denoiser/tests/fixtures/recorded/speech-1.pcm",
            ),
            (
                "speech-2",
                "sotf-plugin-speech-denoiser/tests/fixtures/recorded/speech-2.pcm",
            ),
            (
                "violin",
                "sotf-plugin-de-esser/tests/fixtures/recorded/violin.pcm",
            ),
            (
                "cymbal",
                "sotf-plugin-de-esser/tests/fixtures/recorded/cymbal.pcm",
            ),
        ];
        for (id, path) in fixtures {
            let bytes = std::fs::read(root.join(path)).unwrap();
            let (pcm, remainder) = bytes.as_chunks::<2>();
            assert!(remainder.is_empty());
            let samples: Vec<f32> = pcm
                .iter()
                .map(|v| f32::from(i16::from_le_bytes([v[0], v[1]])) / 32768.0)
                .collect();
            let started = std::time::Instant::now();
            let mut scores = Vec::new();
            for end in (WINDOW_SAMPLES..=samples.len()).step_by(480) {
                let mut input = SourceWindow {
                    samples: [0.0; MAX_SOURCE_SAMPLES],
                    end_sample: end as u64,
                };
                input.samples[..WINDOW_SAMPLES]
                    .copy_from_slice(&samples[end - WINDOW_SAMPLES..end]);
                let observation = model.run(&input).unwrap();
                assert!(observation.score.is_finite() && (0.0..=1.0).contains(&observation.score));
                scores.push(observation.score);
            }
            let elapsed = started.elapsed().as_secs_f64();
            let accepted = scores.iter().filter(|&&score| score >= 0.8).count();
            eprintln!(
                "GATE_RECORDED {}",
                serde_json::json!({
                    "fixture":id,"source_path":path,"source_frames":samples.len(),
                    "observation_window_ms":100,"hop_ms":10,"initial_no_observation_ms":100,
                    "raw_score_threshold":0.8,"windows":scores.len(),"passing_windows":accepted,
                    "passing_fraction":accepted as f64/scores.len() as f64,
                    "mean_score":scores.iter().map(|&x|f64::from(x)).sum::<f64>()/scores.len() as f64,
                    "offline_elapsed_seconds":elapsed,"raw_scores":scores,
                    "limits":"Existing development recordings, unknown model-training overlap, no per-window speech labels. Model-only replay excludes worker deadlines and gain smoothing; passing fraction is not speech recall or population accuracy."
                })
            );
        }
    }

    #[test]
    fn bundled_model_observes_voiced_probes_and_reset_windows_repeat_exactly() {
        let mut backend = RnnoiseBackend::new();
        backend.initialize(48_000, 1).unwrap();
        let mut model = Model {
            backend,
            weights: resampling_weights(48_000.0),
        };
        let mut highest = 0.0_f32;
        for fundamental in [90.0, 120.0, 180.0, 240.0] {
            let mut input = SourceWindow {
                samples: [0.0; MAX_SOURCE_SAMPLES],
                end_sample: 4800,
            };
            for (index, sample) in input.samples[..4800].iter_mut().enumerate() {
                let phase = index as f64 * 2.0 * PI * fundamental / MODEL_RATE;
                *sample = (0.25 * phase.sin() + 0.08 * (2.0 * phase).sin()) as f32;
            }
            let first = model.run(&input).unwrap();
            let repeated = model.run(&input).unwrap();
            assert_eq!(first.score, repeated.score);
            assert_eq!(first.end_sample, input.end_sample);
            highest = highest.max(first.score);
        }
        eprintln!("bundled RNNoise highest synthetic voiced score: {highest}");
        assert!(
            highest >= 0.5,
            "model never supplies usable speech evidence"
        );
    }

    #[test]
    fn policy_counts_the_entire_window_and_rejects_future_stale_and_invalid_scores() {
        let observation = Observation {
            score: 0.95,
            end_sample: 4800,
        };
        assert!(accepted(observation, 5280, 4800, 7200, 0.8));
        for value in [
            Observation {
                end_sample: 6000,
                ..observation
            },
            Observation {
                end_sample: 0,
                ..observation
            },
            Observation {
                score: f32::NAN,
                ..observation
            },
            Observation {
                score: 0.4,
                ..observation
            },
        ] {
            assert!(!accepted(value, 5280, 4800, 7200, 0.8));
        }
    }

    #[test]
    fn resampling_is_causal_dc_normalized_and_bounded_at_supported_rates() {
        for rate in [8000.0, 44100.0, 48000.0, 96000.0, 192000.0] {
            let length = (rate * 0.1_f64).ceil() as usize;
            let weights = resampling_weights(rate);
            assert_eq!(weights.len(), WINDOW_SAMPLES);
            for row in weights {
                assert!(
                    row.iter()
                        .all(|&(index, weight)| index < length && weight.is_finite())
                );
                assert!((row.iter().map(|&(_, weight)| weight).sum::<f64>() - 1.0).abs() < 1e-12);
            }
        }
    }
}
