//! Bounded causal learned proposals layered over the classical AEC post-filter.

// Rust guideline compliant 2026-02-21
use crate::residual_config::{
    ResidualAvailability, ResidualSuppressionConfig, ResidualSuppressionData,
};
use crate::residual_features::{BANDS, BINS, FeatureFrame, ResidualFeatureExtractor, band_range};
use crate::residual_model::ResidualMaskModel;
use plugins_inference::{AsyncInference, InferenceModel};
use rustfft::num_complex::Complex;
use sotf_host::ProcessingMode;
use std::io::Read;

#[derive(Clone, Copy)]
struct Input {
    features: FeatureFrame,
    end_sample: u64,
}
#[derive(Clone, Copy)]
struct Observation {
    gains: Option<[f32; BANDS]>,
    end_sample: u64,
}

struct Model(ResidualMaskModel);
impl InferenceModel for Model {
    type Input = Input;
    type Output = Observation;
    fn run(&mut self, input: &Input) -> Result<Observation, String> {
        Ok(Observation {
            gains: self.0.predict(&input.features),
            end_sample: input.end_sample,
        })
    }
}

enum Backend {
    Realtime(AsyncInference<Model>),
    Offline {
        model: Box<Model>,
        latest: Option<Observation>,
    },
    #[cfg(test)]
    Test {
        latest: Option<Observation>,
        send_ok: bool,
    },
}
impl Backend {
    fn send(&mut self, input: Input) -> bool {
        match self {
            Self::Realtime(worker) => worker.try_send(input).is_ok(),
            Self::Offline { model, latest } => {
                *latest = model.run(&input).ok();
                latest.is_some()
            }
            #[cfg(test)]
            Self::Test { send_ok, .. } => *send_ok,
        }
    }
    fn latest(&self) -> Option<Observation> {
        match self {
            Self::Realtime(worker) if worker.worker_alive() => worker.with_latest(|x| *x),
            Self::Realtime(_) => None,
            Self::Offline { latest, .. } => *latest,
            #[cfg(test)]
            Self::Test { latest, .. } => *latest,
        }
    }
    fn reset(&mut self) {
        match self {
            Self::Realtime(worker) => worker.reset(),
            Self::Offline { latest, .. } => *latest = None,
            #[cfg(test)]
            Self::Test { latest, .. } => *latest = None,
        }
    }
}

pub(crate) struct ResidualBlockContext {
    pub mic_power: f64,
    pub reference_power: f64,
    pub protected: bool,
    pub enabled: bool,
}

pub(crate) struct ResidualStage {
    backend: Backend,
    extractor: ResidualFeatureExtractor,
    end_sample: u64,
    received_frames: u64,
    expected_position: Option<u64>,
    skip_mixed_block: bool,
    clock_exhausted: bool,
    warmup_samples: u64,
    observed_samples: u64,
    max_age_samples: u64,
    last_accepted: Option<u64>,
    extra_floor: f32,
    attack: f32,
    extra_gains: [f32; BINS],
    pub(crate) data: ResidualSuppressionData,
}

impl ResidualStage {
    pub(crate) fn prepare(
        config: &ResidualSuppressionConfig,
        rate: f64,
        mode: ProcessingMode,
    ) -> Result<Self, String> {
        config.validate()?;
        let extractor = ResidualFeatureExtractor::new(rate)?;
        let mut bytes = Vec::new();
        std::fs::File::open(&config.model_path)
            .map_err(|e| e.to_string())?
            .take(262_145)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let model = Model(ResidualMaskModel::from_bytes(&bytes, &config.sha256)?);
        let backend = if mode == ProcessingMode::Offline {
            Backend::Offline {
                model: Box::new(model),
                latest: None,
            }
        } else {
            Backend::Realtime(AsyncInference::spawn(model, 2, "aec-residual")?)
        };
        Ok(Self::with_backend(config, rate, mode, extractor, backend))
    }

    fn with_backend(
        config: &ResidualSuppressionConfig,
        rate: f64,
        mode: ProcessingMode,
        extractor: ResidualFeatureExtractor,
        backend: Backend,
    ) -> Self {
        Self {
            backend,
            extractor,
            end_sample: 0,
            received_frames: 0,
            expected_position: None,
            skip_mixed_block: false,
            clock_exhausted: false,
            warmup_samples: (rate * 0.050).ceil() as u64,
            observed_samples: 0,
            max_age_samples: (rate * f64::from(config.max_age_ms) / 1000.0).floor() as u64,
            last_accepted: None,
            extra_floor: 10.0_f32.powf(-config.max_extra_attenuation_db / 20.0),
            attack: (-256.0 / rate / 0.010).exp() as f32,
            extra_gains: [1.0; BINS],
            data: ResidualSuppressionData {
                availability: ResidualAvailability::Prepared,
                offline: mode == ProcessingMode::Offline,
                ..Default::default()
            },
        }
    }

    pub(crate) fn reset(&mut self) {
        self.backend.reset();
        self.extractor.reset();
        self.end_sample = 0;
        self.observed_samples = 0;
        self.received_frames = 0;
        self.expected_position = None;
        self.last_accepted = None;
        self.skip_mixed_block = false;
        self.clock_exhausted = false;
        self.extra_gains.fill(1.0);
        self.data = ResidualSuppressionData {
            availability: ResidualAvailability::Prepared,
            offline: self.data.offline,
            ..Default::default()
        };
    }

    pub(crate) fn begin_callback(&mut self, position: u64, frames: usize, partial: bool) {
        if frames == 0 {
            return;
        }
        // Legacy callers provide zero on every callback. Use a monotonic local
        // clock until nonzero positions establish an explicit host clock. Such
        // callers must still call Plugin::reset when their source changes.
        let had_host_clock = self.expected_position.is_some();
        let discontinuous = match self.expected_position {
            Some(expected) => position != expected,
            None => position != 0 && self.received_frames != 0 && position != self.received_frames,
        };
        if discontinuous {
            self.reset();
            self.skip_mixed_block = partial;
        }
        if position != 0 || had_host_clock {
            self.expected_position = position.checked_add(frames as u64);
            self.clock_exhausted |= self.expected_position.is_none();
        }
        if let Some(end) = self.received_frames.checked_add(frames as u64) {
            self.received_frames = end;
        } else {
            self.clock_exhausted = true;
        }
    }

    /// Invalidate in-flight work while preserving the last applied coefficients.
    pub(crate) fn freeze(&mut self) {
        self.backend.reset();
    }

    fn observation_gains(&mut self, observation: Option<Observation>) -> Option<[f32; BANDS]> {
        let observation = observation?;
        let age = self.end_sample.checked_sub(observation.end_sample)?;
        if observation.end_sample == 0 || age > self.max_age_samples {
            return None;
        }
        let gains = observation.gains?;
        if gains
            .iter()
            .any(|x| !x.is_finite() || !(0.25..=1.0).contains(x))
        {
            return None;
        }
        self.data.observation_age_samples = Some(age);
        if self.last_accepted != Some(observation.end_sample) {
            self.data.accepted_observations += 1;
            self.last_accepted = Some(observation.end_sample);
        }
        Some(gains)
    }

    pub(crate) fn process_spectrum(
        &mut self,
        error: &[Complex<f32>],
        classical: &mut [Complex<f32>],
        echo: &[Complex<f32>],
        context: ResidualBlockContext,
    ) {
        let ResidualBlockContext {
            mic_power,
            reference_power,
            protected,
            enabled,
        } = context;
        self.data.native_blocks += 1;
        self.data.observation_age_samples = None;
        let Some(end_sample) = self.end_sample.checked_add(256) else {
            self.clock_exhausted = true;
            self.extra_gains.fill(1.0);
            return;
        };
        self.end_sample = end_sample;
        let gains = if self.skip_mixed_block || self.clock_exhausted {
            self.skip_mixed_block = false;
            None
        } else if let Some(features) =
            self.extractor
                .observe(error, echo, mic_power, reference_power)
        {
            self.observed_samples = self.observed_samples.saturating_add(256);
            self.data.warmup_complete = self.observed_samples >= self.warmup_samples;
            if self.backend.send(Input {
                features,
                end_sample,
            }) {
                self.data.submitted_blocks += 1;
                let observation = self.backend.latest();
                if self.data.warmup_complete {
                    self.observation_gains(observation)
                } else {
                    None
                }
            } else {
                self.data.dropped_blocks += 1;
                None
            }
        } else {
            self.backend.reset();
            self.last_accepted = None;
            self.data.warmup_complete = false;
            self.end_sample = 0;
            self.observed_samples = 0;
            None
        };
        let gains = gains.filter(|_| enabled && !protected && reference_power > 1e-10);
        let mut applied = false;
        for band in 0..BANDS {
            for k in band_range(band) {
                let target = if let Some(gains) = &gains {
                    let error_magnitude = error[k].norm();
                    let classical_gain = if error_magnitude > 1e-15 {
                        classical[k].norm() / error_magnitude
                    } else {
                        1.0
                    };
                    (gains[band] / classical_gain.max(1e-15)).clamp(self.extra_floor, 1.0)
                } else {
                    1.0
                };
                let current = self.extra_gains[k];
                let extra = if target < current {
                    self.attack * current + (1.0 - self.attack) * target
                } else {
                    target
                };
                self.extra_gains[k] = extra;
                // Avoid even a multiply-by-one change when falling back.
                if extra < 1.0 {
                    classical[k] *= extra;
                    applied = true;
                }
            }
        }
        if applied {
            self.data.applied_blocks += 1;
        } else {
            self.data.fallback_blocks += 1;
        }
    }

    pub(crate) fn apply_frozen(&self, spectrum: &mut [Complex<f32>]) {
        for (value, &gain) in spectrum.iter_mut().zip(&self.extra_gains) {
            if gain < 1.0 {
                *value *= gain;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::residual_config::RESIDUAL_MODEL_SHA256;

    fn stage() -> ResidualStage {
        let config = ResidualSuppressionConfig {
            model_path: "unused".into(),
            sha256: RESIDUAL_MODEL_SHA256.into(),
            max_age_ms: 40.0,
            max_extra_attenuation_db: 6.0,
        };
        ResidualStage::with_backend(
            &config,
            16_000.0,
            ProcessingMode::Realtime,
            ResidualFeatureExtractor::new(16_000.0).unwrap(),
            Backend::Test {
                latest: None,
                send_ok: true,
            },
        )
    }

    fn tick(stage: &mut ResidualStage, protected: bool) -> [Complex<f32>; BINS] {
        let error = [Complex::new(0.1, 0.0); BINS];
        let mut classical = [Complex::new(0.08, 0.0); BINS];
        stage.process_spectrum(
            &error,
            &mut classical,
            &error,
            ResidualBlockContext {
                mic_power: 0.01,
                reference_power: 0.02,
                protected,
                enabled: true,
            },
        );
        classical
    }

    #[test]
    fn rejects_future_old_invalid_observations_and_full_queue() {
        let mut s = stage();
        s.end_sample = 1024;
        s.observed_samples = 1024;
        assert!(
            s.observation_gains(Some(Observation {
                end_sample: 1025,
                gains: Some([0.25; BANDS])
            }))
            .is_none()
        );
        assert!(
            s.observation_gains(Some(Observation {
                end_sample: 1,
                gains: Some([0.25; BANDS])
            }))
            .is_none()
        );
        assert!(
            s.observation_gains(Some(Observation {
                end_sample: 1024,
                gains: Some([f32::NAN; BANDS])
            }))
            .is_none()
        );
        s.backend = Backend::Test {
            latest: Some(Observation {
                end_sample: 1280,
                gains: Some([0.25; BANDS]),
            }),
            send_ok: false,
        };
        assert_eq!(tick(&mut s, false), [Complex::new(0.08, 0.0); BINS]);
        assert_eq!(s.data.dropped_blocks, 1);
    }

    #[test]
    fn extra_cuts_are_bounded_and_protection_or_missing_result_restores_exact_classical() {
        let mut s = stage();
        s.end_sample = 1024;
        s.observed_samples = 1024;
        s.backend = Backend::Test {
            latest: Some(Observation {
                end_sample: 1280,
                gains: Some([0.25; BANDS]),
            }),
            send_ok: true,
        };
        let result = tick(&mut s, false);
        assert!(
            result
                .iter()
                .all(|x| x.re < 0.08 && x.re >= 0.08 * s.extra_floor)
        );
        let mut frozen = [Complex::new(0.08, 0.0); BINS];
        s.freeze();
        s.apply_frozen(&mut frozen);
        assert_eq!(result, frozen);
        assert_eq!(tick(&mut s, false), [Complex::new(0.08, 0.0); BINS]);
        s.backend = Backend::Test {
            latest: Some(Observation {
                end_sample: 1792,
                gains: Some([0.25; BANDS]),
            }),
            send_ok: true,
        };
        assert_eq!(tick(&mut s, true), [Complex::new(0.08, 0.0); BINS]);
    }

    #[test]
    fn discontinuity_discards_mixed_block_and_reset_forgets_coefficients() {
        let mut s = stage();
        s.begin_callback(0, 128, false);
        s.begin_callback(128, 128, true);
        tick(&mut s, false);
        s.begin_callback(9000, 128, true);
        assert!(s.skip_mixed_block);
        assert_eq!(tick(&mut s, false), [Complex::new(0.08, 0.0); BINS]);
        assert_eq!(s.data.submitted_blocks, 0);
        s.extra_gains.fill(0.5);
        s.reset();
        assert_eq!(s.extra_gains, [1.0; BINS]);
        assert_eq!(s.data.native_blocks, 0);
    }
    #[test]
    fn seek_to_zero_keeps_host_clock_tracking_across_repeated_loops() {
        let mut s = stage();
        s.begin_callback(1000, 256, false);
        tick(&mut s, false);
        s.begin_callback(0, 256, false);
        assert_eq!(s.expected_position, Some(256));
        tick(&mut s, false);
        assert_eq!(s.data.native_blocks, 1);
        s.begin_callback(0, 256, false);
        assert_eq!(s.expected_position, Some(256));
        assert_eq!(s.data.native_blocks, 0);
        assert_eq!(s.end_sample, 0);
    }
}
