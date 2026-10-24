//! Chronological, bounded speech packets with worker-only resampling and state.

use crate::neural_speech::{Observation, accepted};
use crate::silero::SileroModel;
use crate::{SpeechProtectionConfig, SpeechProtectionData, SpeechProtectionModel};
use plugins_inference::{AsyncInference, InferenceModel};
use std::f64::consts::PI;

const MODEL_RATE: f64 = 16000.0;
// Ten milliseconds at the highest supported source rate, 192 kHz.
const MAX_PACKET: usize = 1920;

struct Packet {
    samples: [f32; MAX_PACKET],
    length: usize,
    first_sample: u64,
    generation: u64,
}

#[derive(Clone, Copy, Default)]
struct Resampled {
    value: f32,
    source_end: u64,
}

struct Resampler {
    ring: Vec<f32>,
    received: u64,
    produced: u64,
    origin: u64,
    rate: f64,
    cutoff: f64,
    radius: i64,
}

impl Resampler {
    fn new(rate: f64) -> Result<Self, String> {
        if !rate.is_finite() || !(8000.0..=192000.0).contains(&rate) {
            return Err("Silero speech supports source rates from 8 to 192 kHz".into());
        }
        let cutoff = (MODEL_RATE / rate).min(1.0);
        let radius = (16.0 / cutoff).ceil() as i64;
        Ok(Self {
            ring: vec![0.0; (2 * radius + 4) as usize],
            received: 0,
            produced: 0,
            origin: 0,
            rate,
            cutoff,
            radius,
        })
    }

    fn reset(&mut self, origin: u64) {
        self.ring.fill(0.0);
        self.received = 0;
        self.produced = 0;
        self.origin = origin;
    }

    fn push(&mut self, sample: f32, output: &mut [Resampled; 2]) -> Result<usize, String> {
        let index = (self.received % self.ring.len() as u64) as usize;
        self.ring[index] = sample;
        self.received += 1;
        let mut count = 0;
        loop {
            let position = self.produced as f64 * self.rate / MODEL_RATE;
            if position + self.radius as f64 > (self.received - 1) as f64 {
                break;
            }
            if count >= output.len() {
                return Err("Resampler output bound exceeded".into());
            }
            let center = position.floor() as i64;
            let mut sum = 0.0;
            let mut normalization = 0.0;
            for index in (center - self.radius).max(0)..=center + self.radius {
                if index as u64 >= self.received
                    || self.received - index as u64 > self.ring.len() as u64
                {
                    return Err("Resampler history unavailable".into());
                }
                let distance = index as f64 - position;
                if distance.abs() > self.radius as f64 {
                    continue;
                }
                let phase = PI * distance * self.cutoff;
                let sinc = if phase.abs() < 1e-12 {
                    1.0
                } else {
                    phase.sin() / phase
                };
                let window = 0.42
                    + 0.5 * (PI * distance / self.radius as f64).cos()
                    + 0.08 * (2.0 * PI * distance / self.radius as f64).cos();
                let weight = sinc * self.cutoff * window;
                sum +=
                    f64::from(self.ring[(index as u64 % self.ring.len() as u64) as usize]) * weight;
                normalization += weight;
            }
            output[count] = Resampled {
                value: (sum / normalization) as f32,
                // Timestamp the filter center conservatively. FIR lookahead is
                // already observed input and therefore appears as source age.
                source_end: self.origin + center as u64 + 1,
            };
            count += 1;
            self.produced += 1;
        }
        Ok(count)
    }
}

#[derive(Clone, Copy, Default)]
struct StreamObservation {
    score: Option<f32>,
    end_sample: Option<u64>,
    frames: u64,
    restarts: u64,
}

struct StreamModel {
    net: SileroModel,
    resampler: Resampler,
    state: [f32; 256],
    frame: [f32; 576],
    filled: usize,
    expected: Option<u64>,
    generation: Option<u64>,
    observation: StreamObservation,
}

impl StreamModel {
    fn new(net: SileroModel, rate: f64) -> Result<Self, String> {
        Ok(Self {
            net,
            resampler: Resampler::new(rate)?,
            state: [0.0; 256],
            frame: [0.0; 576],
            filled: 0,
            expected: None,
            generation: None,
            observation: StreamObservation::default(),
        })
    }

    fn restart(&mut self, packet: &Packet) {
        let restarts = if self.generation == Some(packet.generation) {
            self.observation.restarts + u64::from(self.expected.is_some())
        } else {
            0
        };
        self.resampler.reset(packet.first_sample);
        self.state.fill(0.0);
        self.frame.fill(0.0);
        self.filled = 0;
        self.generation = Some(packet.generation);
        self.observation = StreamObservation {
            restarts,
            ..Default::default()
        };
    }

    fn process(&mut self, packet: &Packet) -> Result<StreamObservation, String> {
        if packet.length == 0 || packet.length > MAX_PACKET {
            return Err("Invalid Silero source packet length".into());
        }
        let end = packet
            .first_sample
            .checked_add(packet.length as u64)
            .ok_or("Silero source clock overflow")?;
        if self.generation != Some(packet.generation) || self.expected != Some(packet.first_sample)
        {
            self.restart(packet);
        }
        self.expected = Some(end);
        let mut output = [Resampled::default(); 2];
        for &sample in &packet.samples[..packet.length] {
            let count = self.resampler.push(sample, &mut output)?;
            for sample in &output[..count] {
                self.frame[64 + self.filled] = sample.value;
                self.filled += 1;
                if self.filled == 512 {
                    let (score, state) = self.net.step(&self.frame, &self.state)?;
                    self.state = state;
                    self.frame.copy_within(512..576, 0);
                    self.filled = 0;
                    self.observation.score = Some(score);
                    self.observation.end_sample = Some(sample.source_end);
                    self.observation.frames += 1;
                }
            }
        }
        Ok(self.observation)
    }
}

impl InferenceModel for StreamModel {
    type Input = Packet;
    type Output = StreamObservation;

    fn run(&mut self, packet: &Packet) -> Result<StreamObservation, String> {
        let result = self.process(packet);
        if result.is_err() {
            self.expected = None;
        }
        result
    }
}

pub(super) struct SileroAssistant {
    worker: AsyncInference<StreamModel>,
    config: SpeechProtectionConfig,
    samples: [f32; MAX_PACKET],
    filled: usize,
    hop: usize,
    clock: u64,
    generation: u64,
    latest: Option<StreamObservation>,
    rate: f64,
    age_allowance: u64,
    max_age: u64,
    submitted: u64,
    dropped: u64,
    observed: u64,
}

impl SileroAssistant {
    pub(super) fn new(config: SpeechProtectionConfig, rate: f64) -> Result<Self, String> {
        config.validate()?;
        let SpeechProtectionModel::Silero { model_path, sha256 } = &config.model else {
            return Err("Expected explicit Silero model configuration".into());
        };
        let model = StreamModel::new(SileroModel::load(model_path, sha256)?, rate)?;
        Ok(Self {
            worker: AsyncInference::spawn(model, 2, "gate-silero")?,
            max_age: (rate * f64::from(config.max_age_ms) * 0.001).ceil() as u64,
            config,
            samples: [0.0; MAX_PACKET],
            filled: 0,
            hop: (rate * 0.01).round() as usize,
            clock: 0,
            generation: 0,
            latest: None,
            rate,
            age_allowance: (rate * 0.1).ceil() as u64,
            submitted: 0,
            dropped: 0,
            observed: 0,
        })
    }

    pub(super) fn push(&mut self, sample: f32) {
        self.samples[self.filled] = if sample.is_finite() {
            sample.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        self.filled += 1;
        self.clock = self.clock.saturating_add(1);
        if self.filled == self.hop {
            let packet = Packet {
                samples: self.samples,
                length: self.hop,
                first_sample: self.clock - self.hop as u64,
                generation: self.generation,
            };
            self.filled = 0;
            if self.worker.try_send(packet).is_ok() {
                self.submitted += 1;
            } else {
                self.dropped += 1;
                self.latest = None;
                // Retire queued/in-flight observations immediately. The first
                // subsequent packet exposes the source gap to the worker.
                self.worker.reset();
            }
            if let Some(value) = self.worker.with_latest(|value| *value) {
                if value.end_sample.is_some()
                    && self
                        .latest
                        .is_none_or(|old| old.end_sample != value.end_sample)
                {
                    self.observed += 1;
                }
                self.latest = Some(value);
            }
        }
    }

    pub(super) fn usable(&self) -> bool {
        self.latest.is_some_and(|value| {
            // Four complete hops provide 128 ms of contiguous recurrent input.
            value.frames >= 4
                && value
                    .score
                    .zip(value.end_sample)
                    .is_some_and(|(score, end_sample)| {
                        accepted(
                            Observation { score, end_sample },
                            self.clock,
                            self.age_allowance,
                            self.max_age,
                            self.config.min_probability,
                        )
                    })
        })
    }

    pub(super) fn data(&self, protection_mix: f32) -> SpeechProtectionData {
        let value = self.latest.unwrap_or_default();
        let age = value
            .end_sample
            .and_then(|end| self.clock.checked_sub(end))
            .and_then(|lag| lag.checked_add(self.age_allowance));
        SpeechProtectionData {
            warmup_complete: value.frames >= 4,
            model_frames: value.frames,
            stream_restarts: value.restarts,
            model_score: value.score,
            source_age_ms: age.map(|n| (n as f64 * 1000.0 / self.rate) as f32),
            observation_usable: self.usable(),
            protection_mix,
            submitted_windows: self.submitted,
            dropped_windows: self.dropped,
            observed_windows: self.observed,
        }
    }

    pub(super) fn reset(&mut self) {
        self.worker.reset();
        self.generation = self.generation.wrapping_add(1);
        self.samples.fill(0.0);
        self.filled = 0;
        self.clock = 0;
        self.latest = None;
        self.submitted = 0;
        self.dropped = 0;
        self.observed = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::Deserialize;

    fn net() -> SileroModel {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../models/silero-vad/silero_vad_16k_batch1.onnx");
        SileroModel::load(&path, crate::SILERO_MODEL_SHA256).unwrap()
    }

    #[test]
    fn causal_resampler_preserves_dc_and_source_centers_across_rates() {
        for rate in [
            8000.0, 22050.0, 44100.0, 44123.45, 48000.0, 96000.0, 192000.0,
        ] {
            let mut resampler = Resampler::new(rate).unwrap();
            resampler.reset(12345);
            let mut output = [Resampled::default(); 2];
            let mut count = 0;
            for source in 0..(rate * 0.2) as usize {
                let emitted = resampler.push(0.25, &mut output).unwrap();
                for value in &output[..emitted] {
                    assert!((value.value - 0.25).abs() < 1e-6);
                    assert_eq!(
                        value.source_end,
                        12345 + (count as f64 * rate / MODEL_RATE).floor() as u64 + 1
                    );
                    assert!(value.source_end <= 12345 + source as u64 + 1);
                    count += 1;
                }
            }
            assert!(
                (3160..=3200).contains(&count),
                "rate={rate} emitted={count}"
            );
        }
    }

    #[test]
    fn resampler_tracks_a_passband_tone_and_rejects_downsampling_aliases() {
        for frequency in [1000.0, 12000.0] {
            let mut resampler = Resampler::new(48000.0).unwrap();
            let mut output = [Resampled::default(); 2];
            let mut maximum = 0.0_f64;
            let mut index = 0;
            for source in 0..12000 {
                let signal = (2.0 * PI * frequency * source as f64 / 48000.0).sin() as f32;
                let emitted = resampler.push(signal, &mut output).unwrap();
                for value in &output[..emitted] {
                    if index > 100 {
                        let expected = if frequency < 8000.0 {
                            (2.0 * PI * frequency * index as f64 / MODEL_RATE).sin()
                        } else {
                            0.0
                        };
                        maximum = maximum.max((f64::from(value.value) - expected).abs());
                    }
                    index += 1;
                }
            }
            assert!(maximum < 1e-4, "frequency={frequency} error={maximum}");
        }
    }

    #[derive(Deserialize)]
    struct Golden {
        cases: Vec<Case>,
    }
    #[derive(Deserialize)]
    struct Case {
        fixture: String,
        audio_16k: Vec<f32>,
        step_scores: Vec<f32>,
        final_state: Vec<f32>,
    }

    #[test]
    fn chronological_worker_matches_independent_streaming_states_and_clock() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(
            "../../../../audit/artifacts/ml-gate-silero-20261025/fixed-batch/ort-golden.json",
        );
        let golden: Golden = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        let mut model = StreamModel::new(net(), 16000.0).unwrap();
        let mut score_error = 0.0_f32;
        let mut state_error = 0.0_f32;
        for (generation, case) in golden
            .cases
            .iter()
            .filter(|c| c.fixture.ends_with("-streaming-eight-steps"))
            .enumerate()
        {
            let audio = &case.audio_16k[64..];
            let mut last_frames = 0;
            for first in (0..4160).step_by(160) {
                let mut packet = Packet {
                    samples: [0.0; MAX_PACKET],
                    length: 160,
                    first_sample: first as u64,
                    generation: generation as u64,
                };
                for (index, sample) in packet.samples[..160].iter_mut().enumerate() {
                    *sample = audio.get(first + index).copied().unwrap_or(0.0);
                }
                let observation = model.run(&packet).unwrap();
                if observation.frames != last_frames {
                    let expected = case.step_scores[observation.frames as usize - 1];
                    score_error = score_error.max((observation.score.unwrap() - expected).abs());
                    assert_eq!(observation.end_sample, Some(observation.frames * 512));
                    last_frames = observation.frames;
                }
            }
            assert_eq!(last_frames, 8);
            for (&actual, &expected) in model.state.iter().zip(&case.final_state) {
                state_error = state_error.max((actual - expected).abs());
            }
        }
        eprintln!("Silero chronological score error={score_error}, state error={state_error}");
        assert!(score_error < 1e-4 && state_error < 1e-3);
    }

    #[test]
    fn gap_and_generation_changes_restart_all_recurrent_and_resampler_state() {
        let mut model = StreamModel::new(net(), 48000.0).unwrap();
        for index in 0..30 {
            let packet = Packet {
                samples: [0.1; MAX_PACKET],
                length: 480,
                first_sample: index * 480,
                generation: 0,
            };
            model.run(&packet).unwrap();
        }
        assert!(model.observation.frames >= 4);
        let gap = Packet {
            samples: [0.0; MAX_PACKET],
            length: 480,
            first_sample: 16000,
            generation: 0,
        };
        let observation = model.run(&gap).unwrap();
        assert_eq!(observation.frames, 0);
        assert_eq!(observation.score, None);
        assert_eq!(observation.restarts, 1);
        assert!(model.state.iter().all(|&x| x == 0.0));
        let generation = Packet {
            generation: 1,
            first_sample: 0,
            ..gap
        };
        let observation = model.run(&generation).unwrap();
        assert_eq!(observation.frames, 0);
        assert_eq!(observation.restarts, 0);
        assert_eq!(model.resampler.origin, 0);
    }

    #[test]
    fn arbitrary_packet_partitions_preserve_model_state_and_source_timestamps() {
        for rate in [22050.0, 44123.45, 48000.0, 96000.0] {
            let mut regular = StreamModel::new(net(), rate).unwrap();
            let mut irregular = StreamModel::new(net(), rate).unwrap();
            let length = (rate * 0.25) as usize;
            let source: Vec<_> = (0..length)
                .map(|n| {
                    let phase = 2.0 * PI * 180.0 * n as f64 / rate;
                    (0.2 * phase.sin() + 0.05 * (2.0 * phase).sin()) as f32
                })
                .collect();
            for (model, partitions) in [
                (&mut regular, &[480_usize][..]),
                (&mut irregular, &[1, 137, 511, 1920, 3][..]),
            ] {
                let mut first = 0;
                let mut block = 0;
                while first < length {
                    let count = partitions[block % partitions.len()].min(length - first);
                    let mut packet = Packet {
                        samples: [0.0; MAX_PACKET],
                        length: count,
                        first_sample: first as u64,
                        generation: 7,
                    };
                    packet.samples[..count].copy_from_slice(&source[first..first + count]);
                    model.run(&packet).unwrap();
                    first += count;
                    block += 1;
                }
            }
            assert_eq!(regular.state, irregular.state);
            assert_eq!(regular.frame, irregular.frame);
            assert_eq!(regular.observation.score, irregular.observation.score);
            assert_eq!(
                regular.observation.end_sample,
                irregular.observation.end_sample
            );
            assert_eq!(regular.observation.frames, irregular.observation.frames);
            assert_eq!(irregular.observation.restarts, 0);
        }
    }
}
