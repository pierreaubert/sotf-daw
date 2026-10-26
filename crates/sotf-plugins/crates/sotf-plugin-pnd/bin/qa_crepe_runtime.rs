//! Measure the existing Tract activation API on pre-normalized matched frames.
//!
//! Inference intentionally allocates off the audio thread. Input normalization,
//! source resampling, queues, audio rendering and output decoding are excluded.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use sotf_plugin_pnd::NeuralPitchConfig;
use sotf_plugin_pnd::neural_pitch::CrepeModel;
use std::fs::{File, OpenOptions};
use std::hint::black_box;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::Instant;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    input: PathBuf,
    output: PathBuf,
    report: PathBuf,
    model: PathBuf,
    sha256: String,
    frames: usize,
    warmup_cycles: usize,
    measured_cycles: usize,
}

fn exclusive(path: &PathBuf, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)?
        .write_all(bytes)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let program_start = Instant::now();
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-crepe-runtime REQUEST.json")?;
    let mut bytes = Vec::new();
    File::open(path)?.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err("request exceeds64KiB".into());
    }
    let r: Request = serde_json::from_slice(&bytes)?;
    if !(1..=128).contains(&r.frames)
        || r.warmup_cycles > 100
        || !(1..=100).contains(&r.measured_cycles)
    {
        return Err("unsupported bounded activation corpus/cycles".into());
    }
    bytes.clear();
    File::open(&r.input)?
        .take((r.frames * 1024 * 4 + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() != r.frames * 1024 * 4 {
        return Err("input corpus size mismatch".into());
    }
    let frames: Vec<[f32; 1024]> = bytes
        .as_chunks::<4096>()
        .0
        .iter()
        .map(|frame| {
            std::array::from_fn(|i| {
                f32::from_le_bytes(
                    frame[i * 4..i * 4 + 4]
                        .try_into()
                        .expect("fixed byte chunk"),
                )
            })
        })
        .collect();
    if frames
        .iter()
        .flatten()
        .any(|x| !x.is_finite() || x.abs() > 64.0)
    {
        return Err("invalid finite normalized frames".into());
    }
    let config = NeuralPitchConfig {
        model_path: r.model.to_string_lossy().into_owned(),
        sha256: r.sha256.clone(),
        min_confidence: 0.8,
        max_age_ms: 150.0,
        max_deviation_cents: 100.0,
        source_channel: 0,
    };
    let load_start = Instant::now();
    let mut model = CrepeModel::load(&config, 16000.0)?;
    let model_load_ns = load_start.elapsed().as_nanos() as u64;
    let ready_from_entry_ns = program_start.elapsed().as_nanos() as u64;
    let first_start = Instant::now();
    black_box(model.activations(&frames[0])?);
    let first_inference_ns = first_start.elapsed().as_nanos() as u64;
    for _ in 0..r.warmup_cycles {
        for frame in &frames {
            black_box(model.activations(frame)?);
        }
    }
    let mut outputs = vec![[0.0_f32; 360]; r.frames];
    for (dst, frame) in outputs.iter_mut().zip(&frames) {
        *dst = model.activations(frame)?;
    }
    let mut timings = Vec::with_capacity(r.frames * r.measured_cycles);
    let measurement_start = Instant::now();
    for _ in 0..r.measured_cycles {
        for frame in &frames {
            let start = Instant::now();
            black_box(model.activations(frame)?);
            timings.push(start.elapsed().as_nanos() as u64);
        }
    }
    let measurement_wall_ns = measurement_start.elapsed().as_nanos() as u64;
    let bytes: Vec<u8> = outputs
        .iter()
        .flatten()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    exclusive(&r.output, &bytes)?;
    let report = serde_json::json!({
        "backend": "tract_native", "model_sha256": r.sha256, "frames": r.frames,
        "warmup_cycles": r.warmup_cycles, "measured_cycles": r.measured_cycles,
        "model_load_ns": model_load_ns, "ready_from_entry_ns": ready_from_entry_ns,
        "first_inference_ns": first_inference_ns, "timings_ns": timings,
        "measurement_wall_ns": measurement_wall_ns,
        "thread_policy": "existing production CrepeModel build; internal runtime threading unchanged",
        "scope": "native public activation API includes input Tensor allocation/copy and finite/output validation; no normalization/resampling/worker/audio"
    });
    exclusive(&r.report, &serde_json::to_vec_pretty(&report)?)?;
    Ok(())
}
