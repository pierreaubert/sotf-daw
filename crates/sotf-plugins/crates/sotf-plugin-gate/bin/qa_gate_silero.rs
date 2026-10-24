//! Compare a pinned speech model with independent ONNX Runtime golden states.
//!
//! This diagnostic runs on a control thread. It does not enable Silero in Gate
//! or claim classifier accuracy, audio-callback safety, or deadline compliance.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tract_onnx::prelude::*;

const MODEL_SHA: &str = "7d47a98acce5f4b688a7d541ee0d9061b64171fdb39581cbcf29fe72bfe57c51";

#[derive(Deserialize)]
struct Golden {
    model_sha256: String,
    cases: Vec<Case>,
}

#[derive(Deserialize)]
struct Case {
    fixture: String,
    audio_16k: Vec<f32>,
    step_scores: Vec<f32>,
    final_state: Vec<f32>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut arguments = std::env::args_os().skip(1);
    let model_path = PathBuf::from(arguments.next().ok_or("Missing model path")?);
    let golden_path = PathBuf::from(arguments.next().ok_or("Missing ORT golden path")?);
    if arguments.next().is_some() {
        return Err("Expected model and golden paths only".into());
    }
    let bytes = std::fs::read(model_path)?;
    let actual: String = Sha256::digest(&bytes)
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect();
    if actual != MODEL_SHA {
        return Err("Pinned Silero model SHA-256 mismatch".into());
    }
    let golden: Golden = serde_json::from_slice(&std::fs::read(golden_path)?)?;
    if golden.model_sha256 != actual || golden.cases.is_empty() {
        return Err("Golden model identity mismatch or empty cases".into());
    }
    let model = tract_onnx::onnx()
        .model_for_read(&mut bytes.as_slice())?
        .with_input_fact(0, f32::fact([1, 576]).into())?
        .with_input_fact(1, tensor0(16000_i64).into())?
        .with_input_fact(2, f32::fact([2, 1, 128]).into())?
        .into_optimized()?
        .into_runnable()?;
    let mut score_error = 0.0_f32;
    let mut state_error = 0.0_f32;
    for case in &golden.cases {
        if !(1..=128).contains(&case.step_scores.len())
            || case.audio_16k.len() != 64 + 512 * case.step_scores.len()
            || case.final_state.len() != 256
            || case.audio_16k.iter().any(|v| !v.is_finite())
            || case
                .step_scores
                .iter()
                .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
            || case.final_state.iter().any(|v| !v.is_finite())
        {
            return Err("Invalid golden shape or values".into());
        }
        let mut state = vec![0.0_f32; 256];
        for (step, &expected) in case.step_scores.iter().enumerate() {
            let offset = step * 512;
            let inputs = tvec!(
                Tensor::from_shape(&[1, 576], &case.audio_16k[offset..offset + 576])?.into(),
                tensor0(16000_i64).into(),
                Tensor::from_shape(&[2, 1, 128], &state)?.into(),
            );
            let output = model.run(inputs)?;
            if output.len() != 2 {
                return Err("Unexpected output count".into());
            }
            let score = output[0].to_array_view::<f32>()?;
            let next_state = output[1].to_array_view::<f32>()?;
            if score.shape() != [1, 1] || next_state.shape() != [2, 1, 128] {
                return Err("Unexpected output shape".into());
            }
            let actual = score[[0, 0]];
            if !actual.is_finite() || !(0.0..=1.0).contains(&actual) {
                return Err("Invalid speech score".into());
            }
            score_error = score_error.max((actual - expected).abs());
            for (destination, &value) in state.iter_mut().zip(next_state.iter()) {
                if !value.is_finite() {
                    return Err("Nonfinite recurrent state".into());
                }
                *destination = value;
            }
        }
        for (actual, expected) in state.iter().zip(&case.final_state) {
            state_error = state_error.max((actual - expected).abs());
        }
        eprintln!("Verified {}", case.fixture);
    }
    println!(
        "{}",
        serde_json::json!({
            "model_sha256":actual,"cases":golden.cases.len(),
            "maximum_score_absolute_error":score_error,
            "maximum_state_absolute_error":state_error,
            "score_tolerance":1e-4,"state_tolerance":1e-3,
        "scope":"Independent windows and eight-step streaming probes; native vs ONNX Runtime. No classifier quality or realtime deadline claim."
        })
    );
    if score_error > 1e-4 || state_error > 1e-3 {
        return Err("Native inference disagrees with independent runtime".into());
    }
    Ok(())
}
