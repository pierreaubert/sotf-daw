//! Export causal speech-score receipt times for offline conditioning experiments.
//!
//! Uses the existing public Gate model path in explicit Offline mode. Its 30 Hz
//! diagnostics may repeat between publications; counts identify new snapshots.
//! Scores become available only after each input packet. This is not a realtime
//! inference or sibilance-classification implementation.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use serde_json::json;
use sotf_host::{ParametricInPlacePlugin, ProcessContext, ProcessingMode};
use sotf_plugin_gate::{
    GateData, GatePlugin, GatePluginParams, SpeechProtectionConfig, SpeechProtectionModel,
};
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

// Fixed packet clock matches existing 48 kHz Gate source packetization.
const RATE: u32 = 48_000;
const HOP: usize = 480;
const MAX_FRAMES: usize = 4_000_000;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    input: PathBuf,
    frames: usize,
    model: SpeechProtectionModel,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-speech-observe REQUEST.json")?;
    let mut bytes = Vec::new();
    File::open(path)?.take(65_537).read_to_end(&mut bytes)?;
    if bytes.len() > 65_536 {
        return Err("request exceeds size bound".into());
    }
    let request: Request = serde_json::from_slice(&bytes)?;
    if request.frames == 0 || request.frames > MAX_FRAMES {
        return Err("invalid frame count".into());
    }
    bytes.clear();
    File::open(&request.input)?
        .take((request.frames * 4 + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() != request.frames * 4 {
        return Err("expected mono float32-le input".into());
    }
    let input: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .copied()
        .map(f32::from_le_bytes)
        .collect();
    if input.iter().any(|v| !v.is_finite() || v.abs() > 32.0) {
        return Err("input must be finite within +/-32".into());
    }
    let mut gate = GatePlugin::try_from_params(
        1,
        GatePluginParams {
            speech_protection: Some(SpeechProtectionConfig {
                model: request.model.clone(),
                ..Default::default()
            }),
            mix: 0.0,
            ratio: 1.0,
            link_channels: true,
            lookahead_ms: 0.0,
            ..GatePluginParams::default()
        },
    )?;
    gate.initialize_with_mode(f64::from(RATE), ProcessingMode::Offline)?;
    let mut buffer = vec![0.0; HOP];
    let mut rows = Vec::with_capacity(request.frames.div_ceil(HOP));
    let mut observed = 0;
    let mut elapsed = 0;
    for source in input.chunks(HOP) {
        buffer[..source.len()].copy_from_slice(source);
        let context = ProcessContext::new(RATE, source.len()).with_sample_position(elapsed as u64);
        if gate.process_in_place(&mut buffer[..source.len()], &context)? != source.len() {
            return Err("native Gate changed frame count".into());
        }
        elapsed += source.len();
        let data = gate
            .get_data()
            .ok_or("missing Gate diagnostics")?
            .downcast::<GateData>()
            .map_err(|_| "unexpected Gate data")?;
        let status = data.speech_protection.ok_or("missing speech diagnostics")?;
        let new_observation = status.observed_windows != observed;
        observed = status.observed_windows;
        rows.push(json!({"available_at_frame":elapsed,"new_observation":new_observation,
            "score":status.model_score,"warmup_complete":status.warmup_complete,
            "source_age_ms_at_snapshot":status.source_age_ms,"observed":observed,
            "model_frames":status.model_frames,"dropped":status.dropped_windows,"restarts":status.stream_restarts}));
    }
    println!(
        "{}",
        json!({"status":"ok","sample_rate":RATE,"input_frames":request.frames,
        "packet_frames":HOP,"model":request.model,"rows":rows,
        "scope":"Explicit Offline Gate inference; public diagnostic snapshot receipt times only. Scores are not calibrated sibilance probabilities; no retrospective application or EOF padding."})
    );
    Ok(())
}
