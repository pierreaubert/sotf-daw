//! Prepared model dispatch; the legacy RNNoise choice registry remains stable.

use crate::{DeepFilterConfig, SpeechDenoiserData, SpeechDenoiserModel};
use plugins_denoiser::rnnoise::RnnoiseBackend;

/// DeepFilter worker telemetry; local SNR is not a speech probability.
#[derive(Debug, Clone, Copy, Default)]
pub struct DeepFilterData {
    /// Fixed model-clock delay including the configured worker playout budget.
    pub model_clock_latency_samples: usize,
    /// Timely model packets selected for playout, including bypassed packets.
    pub timely_packets: u64,
    /// Scheduled model packets replaced with correctly delayed source audio.
    pub fallback_packets: u64,
    /// Packets with a dry prefix while fresh recurrent history fills.
    pub warmup_packets: u64,
    /// Input packets rejected by bounded worker backpressure.
    pub dropped_inputs: u64,
    /// Lifetime model errors; recurrent state is restored after each failure.
    pub lifetime_worker_errors: u64,
    /// Lifetime restorations of all recurrent and normalization state.
    pub lifetime_worker_resets: u64,
    /// Local-SNR score from the last timely packet, in dB.
    pub last_local_snr_db: Option<f32>,
}

pub(super) trait FrameBackend {
    fn process(&mut self, audio: &mut [f32], frames: usize, channels: usize, bypass: bool)
    -> usize;
}

impl FrameBackend for RnnoiseBackend {
    fn process(
        &mut self,
        audio: &mut [f32],
        frames: usize,
        channels: usize,
        bypass: bool,
    ) -> usize {
        RnnoiseBackend::process(self, audio, frames, channels, bypass)
    }
}

pub(super) enum Backend {
    Rnnoise(Box<RnnoiseBackend>),
    #[cfg(feature = "deep-filter")]
    DeepFilter(Box<plugins_denoiser::deep_filter::DeepFilterStream>),
}

pub(super) fn validate_request(
    channels: usize,
    config: Option<&DeepFilterConfig>,
) -> Result<(), String> {
    if let Some(config) = config {
        config.validate()?;
        if !(1..=2).contains(&channels) {
            return Err("DeepFilterNet2 requires mono or linked stereo".into());
        }
        #[cfg(not(feature = "deep-filter"))]
        return Err("DeepFilterNet2 requires the deep-filter build feature".into());
    }
    Ok(())
}

impl Backend {
    pub(super) fn new() -> Self {
        Self::Rnnoise(Box::new(RnnoiseBackend::new()))
    }

    pub(super) fn prepare(
        channels: usize,
        model: SpeechDenoiserModel,
        config: Option<&DeepFilterConfig>,
    ) -> Result<Self, String> {
        validate_request(channels, config)?;
        #[cfg(feature = "deep-filter")]
        if let Some(config) = config {
            return plugins_denoiser::deep_filter::prepare_stream_for_channels(config, channels)
                .map(|stream| Self::DeepFilter(Box::new(stream)));
        }
        let mut backend = RnnoiseBackend::new();
        backend.initialize_with_model(48_000, channels, model.backend_id())?;
        Ok(Self::Rnnoise(Box::new(backend)))
    }

    pub(super) fn latency_samples(&self) -> usize {
        match self {
            Self::Rnnoise(backend) => backend.latency_samples(),
            #[cfg(feature = "deep-filter")]
            Self::DeepFilter(backend) => backend.latency_samples(),
        }
    }

    pub(super) fn reset(&mut self) {
        match self {
            Self::Rnnoise(backend) => backend.reset(),
            #[cfg(feature = "deep-filter")]
            Self::DeepFilter(backend) => backend.reset(),
        }
    }

    pub(super) fn analyzer_data(&self) -> Option<SpeechDenoiserData> {
        match self {
            Self::Rnnoise(backend) => Some(backend.analyzer_data()),
            #[cfg(feature = "deep-filter")]
            Self::DeepFilter(_) => None,
        }
    }

    pub(super) fn deep_filter_data(&self) -> Option<DeepFilterData> {
        match self {
            Self::Rnnoise(_) => None,
            #[cfg(feature = "deep-filter")]
            Self::DeepFilter(backend) => {
                let status = backend.status();
                Some(DeepFilterData {
                    model_clock_latency_samples: backend.latency_samples(),
                    timely_packets: status.model_packets,
                    fallback_packets: status.fallback_packets,
                    warmup_packets: status.warmup_packets,
                    dropped_inputs: status.dropped_inputs,
                    lifetime_worker_errors: status.worker_errors,
                    lifetime_worker_resets: status.worker_resets,
                    last_local_snr_db: status.last_score,
                })
            }
        }
    }
}

impl FrameBackend for Backend {
    fn process(
        &mut self,
        audio: &mut [f32],
        frames: usize,
        channels: usize,
        bypass: bool,
    ) -> usize {
        match self {
            Self::Rnnoise(backend) => backend.process(audio, frames, channels, bypass),
            #[cfg(feature = "deep-filter")]
            Self::DeepFilter(backend) => {
                let Some(samples) = frames.checked_mul(channels) else {
                    return 0;
                };
                if channels != backend.channels() || samples > audio.len() {
                    return 0;
                }
                if backend.process(&mut audio[..samples], bypass).is_ok() {
                    frames
                } else {
                    0
                }
            }
        }
    }
}
