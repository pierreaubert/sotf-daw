//! Fixed-contract Silero inference, restricted to control and worker threads.

use crate::SILERO_MODEL_SHA256;
use sha2::{Digest, Sha256};
use std::io::Read;
use std::path::Path;
use tract_onnx::prelude::*;

/// A verified model plan; recurrent state is explicitly owned by its caller.
///
/// Loading and inference allocate and must never execute on an audio callback.
pub struct SileroModel {
    model: std::sync::Arc<TypedRunnableModel>,
}

impl std::fmt::Debug for SileroModel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SileroModel")
            .field("sha256", &SILERO_MODEL_SHA256)
            .finish_non_exhaustive()
    }
}

impl SileroModel {
    /// Load the pinned fixed-batch model, verifying its identity and contract.
    ///
    /// # Errors
    /// Rejects unreadable, oversized, altered, unsupported, or unrunnable models.
    pub fn load(path: &Path, expected_sha: &str) -> Result<Self, String> {
        if expected_sha != SILERO_MODEL_SHA256 {
            return Err("Pinned Silero model SHA-256 mismatch".into());
        }
        // The verified model is under 3 MiB; bound reads before parsing ONNX.
        const MAX_BYTES: u64 = 4 * 1024 * 1024;
        let mut bytes = Vec::new();
        std::fs::File::open(path)
            .map_err(|e| e.to_string())?
            .take(MAX_BYTES + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| e.to_string())?;
        let actual: String = Sha256::digest(&bytes)
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect();
        if bytes.len() as u64 > MAX_BYTES || actual != expected_sha {
            return Err("Pinned Silero model SHA-256 mismatch".into());
        }
        let model = tract_onnx::onnx()
            .model_for_read(&mut bytes.as_slice())
            .map_err(|e| e.to_string())?
            .with_input_fact(0, f32::fact([1, 576]).into())
            .map_err(|e| e.to_string())?
            .with_input_fact(1, tensor0(16000_i64).into())
            .map_err(|e| e.to_string())?
            .with_input_fact(2, f32::fact([2, 1, 128]).into())
            .map_err(|e| e.to_string())?
            .into_optimized()
            .map_err(|e| e.to_string())?
            .into_runnable()
            .map_err(|e| e.to_string())?;
        let mut result = Self { model };
        result.step(&[0.0; 576], &[0.0; 256])?;
        Ok(result)
    }

    /// Evaluate 64 context and 512 new samples at 16 kHz, with explicit state.
    ///
    /// This method allocates; call it only on a control or inference worker thread.
    ///
    /// # Errors
    /// Rejects nonfinite inputs, inference failures, and invalid outputs or state.
    pub fn step(
        &mut self,
        input: &[f32; 576],
        state: &[f32; 256],
    ) -> Result<(f32, [f32; 256]), String> {
        if input.iter().chain(state.iter()).any(|v| !v.is_finite()) {
            return Err("Nonfinite Silero input or state".into());
        }
        let output = self
            .model
            .run(tvec!(
                Tensor::from_shape(&[1, 576], input)
                    .map_err(|e| e.to_string())?
                    .into(),
                tensor0(16000_i64).into(),
                Tensor::from_shape(&[2, 1, 128], state)
                    .map_err(|e| e.to_string())?
                    .into(),
            ))
            .map_err(|e| e.to_string())?;
        if output.len() != 2 {
            return Err("Unexpected Silero output count".into());
        }
        let score = output[0]
            .to_plain_array_view::<f32>()
            .map_err(|e| e.to_string())?;
        let state = output[1]
            .to_plain_array_view::<f32>()
            .map_err(|e| e.to_string())?;
        if score.shape() != [1, 1] || state.shape() != [2, 1, 128] {
            return Err("Unexpected Silero output shape".into());
        }
        let value = score[[0, 0]];
        if !value.is_finite()
            || !(0.0..=1.0).contains(&value)
            || state.iter().any(|v| !v.is_finite())
        {
            return Err("Invalid Silero score or state".into());
        }
        let mut next = [0.0; 256];
        for (destination, &value) in next.iter_mut().zip(state.iter()) {
            *destination = value;
        }
        Ok((value, next))
    }
}
