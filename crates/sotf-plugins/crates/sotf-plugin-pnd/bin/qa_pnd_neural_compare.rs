//! Matched source-clock estimator observations and paced native PND rendering.
//!
//! Raw model execution precedes pacing. Worker-assisted and classical plugins
//! consume identical callbacks on one source clock; neither waits for inference.

// Rust guideline compliant 2026-02-21
use serde::{Deserialize, Serialize};
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, Plugin, ProcessContext};
use sotf_plugin_pnd::analysis::PndAnalyzer;
use sotf_plugin_pnd::neural_pitch::CrepeModel;
use sotf_plugin_pnd::{NeuralPitchConfig, PitchDecisionSnapshot, PndPlugin, PndPluginParams};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Request {
    input: PathBuf,
    classical_output: PathBuf,
    neural_output: PathBuf,
    report: PathBuf,
    frames: usize,
    sample_rate: u32,
    reference_hz: f32,
    model: NeuralPitchConfig,
    neural_first: bool,
}

#[derive(Debug, Serialize)]
struct RawPitch {
    source_end_frame: usize,
    window_frames: usize,
    frequency_hz: f64,
    confidence: f32,
}

#[derive(Debug, Serialize)]
struct Trace {
    source_end_frame: usize,
    release_ns: u64,
    start_lateness_ns: u64,
    classical_execution_ns: u64,
    neural_execution_ns: u64,
    pair_response_ns: u64,
    classical: PitchDecisionSnapshot,
    neural: PitchDecisionSnapshot,
}

fn exclusive(path: &PathBuf, bytes: &[u8]) -> Result<(), Box<dyn std::error::Error>> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)?
        .write_all(bytes)?;
    Ok(())
}

fn render_block(
    plugin: &mut PndPlugin,
    input: &[f32],
    output: &mut [f32],
    rate: u32,
) -> Result<u64, String> {
    let start = Instant::now();
    let mut result = Ok(0);
    assert_no_allocs_or_deallocs("PND matched process", || {
        result = plugin.process(input, output, &ProcessContext::new(rate, input.len()));
    });
    if result? != input.len() {
        return Err("PND did not preserve frame count".into());
    }
    Ok(start.elapsed().as_nanos() as u64)
}

fn drain(plugin: &mut PndPlugin, output: &mut Vec<f32>, rate: u32) -> Result<(), String> {
    let before = plugin.pitch_decision_snapshot();
    let mut block = [0.0_f32; 128];
    let block_frames = block.len();
    for _ in 0..128 {
        let mut result = None;
        assert_no_allocs_or_deallocs("PND matched drain", || {
            result = Some(plugin.drain(&mut block, &ProcessContext::new(rate, block_frames)));
        });
        let result = result.ok_or("unexecuted drain")??;
        output.extend_from_slice(&block[..result.frames]);
        if result.complete {
            if before != plugin.pitch_decision_snapshot() {
                return Err("PND decisions advanced during finite drain".into());
            }
            return Ok(());
        }
    }
    Err("PND drain exceeded bounded tail".into())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-pnd-neural-compare REQUEST.json")?;
    let mut bytes = Vec::new();
    File::open(path)?.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err("request exceeds64KiB".into());
    }
    let r: Request = serde_json::from_slice(&bytes)?;
    if ![44100, 48000, 96000].contains(&r.sample_rate)
        || !(2048..=8 * r.sample_rate as usize).contains(&r.frames)
        || !r.reference_hz.is_finite()
        || !(65.0..=1000.0).contains(&r.reference_hz)
        || r.model.source_channel != 0
    {
        return Err("unsupported bounded mono PND geometry".into());
    }
    bytes.clear();
    File::open(&r.input)?
        .take((r.frames * 4 + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() != r.frames * 4 {
        return Err("input size mismatch".into());
    }
    let input: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| f32::from_le_bytes(*v))
        .collect();
    if input.iter().any(|v| !v.is_finite() || v.abs() > 4.0) {
        return Err("invalid bounded input".into());
    }
    let mut raw_classical = Vec::with_capacity(r.frames / 512);
    let mut analyzer = PndAnalyzer::new(2048, f64::from(r.sample_rate), 100.0);
    let mut previous = 0;
    for (i, sample) in input.iter().enumerate() {
        analyzer.analyze(std::slice::from_ref(sample));
        if analyzer.analysis_generation() != previous {
            previous = analyzer.analysis_generation();
            let (ratio, confidence) = analyzer.estimate_against_reference(r.reference_hz);
            raw_classical.push(RawPitch {
                source_end_frame: i + 1,
                window_frames: 2048,
                frequency_hz: f64::from(ratio) * f64::from(r.reference_hz),
                confidence,
            });
        }
    }
    let mut raw_crepe = Vec::with_capacity(r.frames / 480);
    let window = (0.064 * f64::from(r.sample_rate)).ceil() as usize;
    let hop = (f64::from(r.sample_rate) / 100.0).round() as usize;
    let mut model = CrepeModel::load(&r.model, f64::from(r.sample_rate))?;
    for end in (window..=r.frames).step_by(hop) {
        let p = model.analyze_window(&input[end - window..end], end as u64)?;
        raw_crepe.push(RawPitch {
            source_end_frame: end,
            window_frames: window,
            frequency_hz: f64::from(p.frequency_hz),
            confidence: p.confidence,
        });
    }
    drop(model);
    let params = PndPluginParams {
        reference_frequency_hz: r.reference_hz,
        confidence_threshold: 0.5,
        drift_smoothing: 0.1,
        multi_channel_analysis: false,
        correction_strength: 1.0,
        ..PndPluginParams::default()
    };
    let mut classical = PndPlugin::from_params(1, params.clone())?;
    let mut neural = PndPlugin::from_params(
        1,
        PndPluginParams {
            neural_pitch: Some(r.model.clone()),
            ..params.clone()
        },
    )?;
    classical.initialize(f64::from(r.sample_rate))?;
    neural.initialize(f64::from(r.sample_rate))?;
    assert_no_allocs_or_deallocs("PND matched reset", || {
        classical.reset();
        neural.reset();
    });
    if classical.latency_samples() != 2047 || neural.latency_samples() != 2047 {
        return Err("unexpected PND latency".into());
    }
    let mut a = Vec::with_capacity(r.frames + 16384);
    let mut b = Vec::with_capacity(r.frames + 16384);
    let mut ab = [0.0_f32; 128];
    let mut bb = ab;
    let mut traces = Vec::with_capacity(r.frames.div_ceil(128));
    let clock = Instant::now();
    let mut accepted = 0;
    for source in input.chunks(128) {
        let release_ns = (accepted as u64 * 1_000_000_000) / u64::from(r.sample_rate);
        let release = clock + Duration::from_nanos(release_ns);
        if let Some(remaining) = release.checked_duration_since(Instant::now()) {
            std::thread::sleep(remaining);
        }
        let start_lateness_ns = Instant::now().saturating_duration_since(release).as_nanos() as u64;
        let n = source.len();
        let (ca, cb) = if r.neural_first {
            let cb = render_block(&mut neural, source, &mut bb[..n], r.sample_rate)?;
            (
                render_block(&mut classical, source, &mut ab[..n], r.sample_rate)?,
                cb,
            )
        } else {
            let ca = render_block(&mut classical, source, &mut ab[..n], r.sample_rate)?;
            (
                ca,
                render_block(&mut neural, source, &mut bb[..n], r.sample_rate)?,
            )
        };
        let response = Instant::now().saturating_duration_since(release).as_nanos() as u64;
        a.extend_from_slice(&ab[..n]);
        b.extend_from_slice(&bb[..n]);
        accepted += n;
        traces.push(Trace {
            source_end_frame: accepted,
            release_ns,
            start_lateness_ns,
            classical_execution_ns: ca,
            neural_execution_ns: cb,
            pair_response_ns: response,
            classical: classical.pitch_decision_snapshot(),
            neural: neural.pitch_decision_snapshot(),
        });
    }
    drain(&mut classical, &mut a, r.sample_rate)?;
    drain(&mut neural, &mut b, r.sample_rate)?;
    if a.len() != b.len() || a.iter().chain(&b).any(|v| !v.is_finite()) {
        return Err("output geometry or finite-domain mismatch".into());
    }
    for (path, audio) in [(&r.classical_output, &a), (&r.neural_output, &b)] {
        let bytes: Vec<u8> = audio.iter().flat_map(|x| x.to_le_bytes()).collect();
        exclusive(path, &bytes)?;
    }
    let report = serde_json::json!({"sample_rate": r.sample_rate, "source_frames": r.frames, "output_frames": a.len(), "latency_samples": 2047, "callback_frames": 128, "params": params, "neural_config": r.model, "neural_first": r.neural_first, "raw_classical": raw_classical, "raw_crepe": raw_crepe, "trace": traces, "worker_timestamp_available": false, "scope": "paced mono native pair; raw synchronous estimates separate; no device scheduling or inference-job age telemetry"});
    exclusive(&r.report, &serde_json::to_vec_pretty(&report)?)?;
    Ok(())
}
