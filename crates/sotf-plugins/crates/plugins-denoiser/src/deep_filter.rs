//! Pinned DeepFilterNet2 adapters and chronological mono/stereo workers.
//!
//! A pristine clone restores STFT memories, recurrent state, rolling spectra,
//! and feature normalization. Preparation, inference, reset cloning, and model
//! destruction belong exclusively on the worker. Linked stereo shares ERB masks,
//! complex deep-filter coefficients, and stage decisions without mixing audio.

use crate::deep_filter_config::DeepFilterConfig;
use deep_filter_runtime::tract::{DfParams, DfTract, RuntimeParams};
use ndarray017::{ArrayView2, ArrayViewMut2};
use plugins_inference::framed_audio::{AudioFrameModel, AudioInferenceStatus, FramedAudio};
use sha2::{Digest, Sha256};

/// Fixed 10 ms packet at the model's 48 kHz clock.
pub const DEEP_FILTER_HOP: usize = 480;
const STEREO_PACKET_SAMPLES: usize = DEEP_FILTER_HOP * 2;
/// Intrinsic STFT and DFN2 lookahead delay in frames per channel.
pub const DEEP_FILTER_LATENCY: usize = 2400;

struct PreparedModel {
    pristine: DfTract,
    current: DfTract,
    channels: usize,
    latency: usize,
}

impl std::fmt::Debug for PreparedModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedDeepFilter")
            .field("channels", &self.channels)
            .field("latency", &self.latency)
            .finish_non_exhaustive()
    }
}

impl PreparedModel {
    fn load(config: &DeepFilterConfig, channels: usize) -> Result<Self, String> {
        config.validate()?;
        let bytes = std::fs::read(&config.model_path).map_err(|e| e.to_string())?;
        let actual: String = Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        if actual != config.sha256 {
            return Err("DeepFilter model SHA-256 mismatch".into());
        }
        let params = DfParams::from_bytes(&bytes).map_err(|e| format!("{e:#}"))?;
        let pristine = DfTract::new(params, &RuntimeParams::default_with_ch(channels))
            .map_err(|e| format!("{e:#}"))?;
        if pristine.sr != 48_000
            || pristine.ch != channels
            || pristine.hop_size != DEEP_FILTER_HOP
            || pristine.fft_size != 960
            || pristine.nb_erb != 32
            || pristine.nb_df != 96
            || pristine.df_order != 5
            || pristine.lookahead != 4
        {
            return Err("Unsupported DeepFilterNet2 model geometry".into());
        }
        let latency =
            pristine.fft_size - pristine.hop_size + pristine.lookahead * pristine.hop_size;
        let current = pristine.clone();
        Ok(Self {
            pristine,
            current,
            channels,
            latency,
        })
    }

    fn reset(&mut self) {
        self.current = self.pristine.clone();
    }

    fn process_planar(&mut self, input: &[f32], output: &mut [f32]) -> Result<f32, String> {
        let shape = (self.channels, DEEP_FILTER_HOP);
        let noisy = ArrayView2::from_shape(shape, input).map_err(|e| e.to_string())?;
        let enhanced = ArrayViewMut2::from_shape(shape, output).map_err(|e| e.to_string())?;
        let score = self
            .current
            .process(noisy, enhanced)
            .map_err(|e| format!("{e:#}"))?;
        if !score.is_finite() || output.iter().any(|x| !x.is_finite()) {
            return Err("DeepFilter emitted non-finite audio or local-SNR score".into());
        }
        Ok(score)
    }
}

/// Prepared mono DeepFilterNet2 model with a pristine recurrent-state snapshot.
#[derive(Debug)]
pub struct DeepFilterModel(PreparedModel);

/// Prepared linked-stereo DeepFilterNet2 model with independent audio histories.
///
/// Both channels receive the mean ERB mask, mean complex FIR coefficients, and
/// mean local-SNR stage decision. This is a linked extension of the mono model,
/// not a model trained to separate or remix spatial sources.
#[derive(Debug)]
pub struct LinkedDeepFilterModel(PreparedModel);

macro_rules! impl_model {
    ($model:ident, $channels:literal, $samples:expr) => {
        impl $model {
            /// Load a verified archive and prepare all channel state off audio.
            ///
            /// # Errors
            /// Rejects invalid policy, unreadable or mismatched weights,
            /// unsupported geometry, and runtime preparation failures.
            pub fn load(config: &DeepFilterConfig) -> Result<Self, String> {
                PreparedModel::load(config, $channels).map(Self)
            }
        }

        impl AudioFrameModel<$samples> for $model {
            fn channels(&self) -> usize {
                $channels
            }
            fn latency_samples(&self) -> usize {
                self.0.latency
            }
            fn reset(&mut self) -> Result<(), String> {
                self.0.reset();
                Ok(())
            }
            fn process(
                &mut self,
                input: &[f32; $samples],
                output: &mut [f32; $samples],
            ) -> Result<f32, String> {
                let mut planar = [0.0; $samples];
                for frame in 0..DEEP_FILTER_HOP {
                    for channel in 0..$channels {
                        planar[channel * DEEP_FILTER_HOP + frame] =
                            input[frame * $channels + channel];
                    }
                }
                let mut enhanced = [0.0; $samples];
                let score = self.0.process_planar(&planar, &mut enhanced)?;
                for frame in 0..DEEP_FILTER_HOP {
                    // A common full-scale bound prevents the transport's final
                    // safety clamp from independently clipping stereo channels.
                    let mut peak = 1.0_f32;
                    if $channels > 1 {
                        for channel in 0..$channels {
                            peak = peak.max(enhanced[channel * DEEP_FILTER_HOP + frame].abs());
                        }
                    }
                    for channel in 0..$channels {
                        output[frame * $channels + channel] =
                            enhanced[channel * DEEP_FILTER_HOP + frame] / peak;
                    }
                }
                Ok(score)
            }
        }
    };
}
impl_model!(DeepFilterModel, 1, DEEP_FILTER_HOP);
impl_model!(LinkedDeepFilterModel, 2, STEREO_PACKET_SAMPLES);

#[derive(Debug)]
enum StreamInner {
    Mono(Box<FramedAudio<DeepFilterModel, DEEP_FILTER_HOP>>),
    Stereo(Box<FramedAudio<LinkedDeepFilterModel, STEREO_PACKET_SAMPLES>>),
}

/// Audio-side mono/stereo stream with fixed playout and delayed-source fallback.
#[derive(Debug)]
pub struct DeepFilterStream {
    inner: StreamInner,
}

impl DeepFilterStream {
    /// Return the prepared channel count and interleaving contract.
    pub fn channels(&self) -> usize {
        match self.inner {
            StreamInner::Mono(_) => 1,
            StreamInner::Stereo(_) => 2,
        }
    }

    /// Return fixed total latency in frames per channel.
    pub fn latency_samples(&self) -> usize {
        match &self.inner {
            StreamInner::Mono(stream) => stream.latency_samples(),
            StreamInner::Stereo(stream) => stream.latency_samples(),
        }
    }

    /// Read model-specific packet counters without worker locks.
    pub fn status(&self) -> AudioInferenceStatus {
        match &self.inner {
            StreamInner::Mono(stream) => stream.status(),
            StreamInner::Stereo(stream) => stream.status(),
        }
    }

    /// Clear source history and reject old worker generations without waiting.
    pub fn reset(&mut self) {
        match &mut self.inner {
            StreamInner::Mono(stream) => stream.reset(),
            StreamInner::Stereo(stream) => stream.reset(),
        }
    }

    /// Overwrite interleaved audio with bounded, nonblocking model playout.
    ///
    /// # Errors
    /// Rejects incomplete interleaved channel frames before mutating state.
    pub fn process(&mut self, audio: &mut [f32], bypass: bool) -> Result<(), String> {
        match &mut self.inner {
            StreamInner::Mono(stream) => stream.process(audio, bypass),
            StreamInner::Stereo(stream) => stream.process(audio, bypass),
        }
    }
}

/// Prepare a verified mono model and its chronological worker off audio.
///
/// # Errors
/// Returns model, storage, geometry, or worker preparation failures.
pub fn prepare_stream(config: &DeepFilterConfig) -> Result<DeepFilterStream, String> {
    prepare_stream_for_channels(config, 1)
}

/// Prepare mono or linked stereo with fixed per-channel latency off audio.
///
/// # Errors
/// Rejects wider layouts and model, storage, geometry, or worker preparation failures.
pub fn prepare_stream_for_channels(
    config: &DeepFilterConfig,
    channels: usize,
) -> Result<DeepFilterStream, String> {
    config.validate()?;
    let owned = config.clone();
    let inner = match channels {
        1 => StreamInner::Mono(Box::new(FramedAudio::with_factory(
            1,
            DEEP_FILTER_LATENCY,
            config.playout_packets,
            move || DeepFilterModel::load(&owned),
        )?)),
        2 => StreamInner::Stereo(Box::new(FramedAudio::with_factory(
            2,
            DEEP_FILTER_LATENCY,
            config.playout_packets,
            move || LinkedDeepFilterModel::load(&owned),
        )?)),
        _ => return Err("DeepFilterNet2 requires mono or linked stereo".into()),
    };
    Ok(DeepFilterStream { inner })
}
