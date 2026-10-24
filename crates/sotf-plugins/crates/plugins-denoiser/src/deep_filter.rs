//! Pinned DeepFilterNet2 adapter; recurrent execution belongs on a framed worker.
//!
//! Upstream `DfTract::init` alone does not restore every recurrent and feature
//! normalization field. A pristine clone restores the entire prepared state,
//! including STFT memories, pulse states, rolling spectra, and normalization.
//! Construction, cloning, inference, and destruction must remain off audio.

use crate::deep_filter_config::DeepFilterConfig;
use deep_filter_runtime::tract::{DfParams, DfTract, RuntimeParams};
use ndarray015::{ArrayView2, ArrayViewMut2};
use plugins_inference::framed_audio::{AudioFrameModel, FramedAudio};
use sha2::{Digest, Sha256};

/// Fixed 10 ms packet at the model's 48 kHz clock.
pub const DEEP_FILTER_HOP: usize = 480;

/// Prepared mono DeepFilterNet2 model with a pristine recurrent-state snapshot.
///
/// Linked stereo is a separate required integration step; this adapter refuses
/// wider layouts rather than independently altering channel transfer functions.
pub struct DeepFilterModel {
    pristine: DfTract,
    current: DfTract,
    latency: usize,
}

impl std::fmt::Debug for DeepFilterModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DeepFilterModel")
            .field("channels", &1)
            .field("latency", &self.latency)
            .finish_non_exhaustive()
    }
}

impl DeepFilterModel {
    /// Load a verified fixed-shape DeepFilterNet2 ONNX archive.
    ///
    /// # Errors
    /// Rejects invalid policy, unreadable or mismatched archives, unsupported
    /// model geometry, and upstream model preparation failures.
    pub fn load(config: &DeepFilterConfig) -> Result<Self, String> {
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
        let pristine = DfTract::new(params, &RuntimeParams::default_with_ch(1))
            .map_err(|e| format!("{e:#}"))?;
        if pristine.sr != 48_000
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
            latency,
        })
    }
}

impl AudioFrameModel<DEEP_FILTER_HOP> for DeepFilterModel {
    fn channels(&self) -> usize {
        1
    }
    fn latency_samples(&self) -> usize {
        self.latency
    }
    fn reset(&mut self) -> Result<(), String> {
        self.current = self.pristine.clone();
        Ok(())
    }
    fn process(
        &mut self,
        input: &[f32; DEEP_FILTER_HOP],
        output: &mut [f32; DEEP_FILTER_HOP],
    ) -> Result<f32, String> {
        let noisy =
            ArrayView2::from_shape((1, DEEP_FILTER_HOP), input).map_err(|e| e.to_string())?;
        let enhanced =
            ArrayViewMut2::from_shape((1, DEEP_FILTER_HOP), output).map_err(|e| e.to_string())?;
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

/// Audio-side stream with fixed playout and exact delayed-source fallback.
pub type DeepFilterStream = FramedAudio<DeepFilterModel, DEEP_FILTER_HOP>;

/// Prepare a verified mono model and its chronological worker off audio.
///
/// # Errors
/// Returns model validation, preparation, allocation, or worker creation failures.
pub fn prepare_stream(config: &DeepFilterConfig) -> Result<DeepFilterStream, String> {
    config.validate()?;
    let owned = config.clone();
    FramedAudio::with_factory(1, 2400, config.playout_packets, move || {
        DeepFilterModel::load(&owned)
    })
}
