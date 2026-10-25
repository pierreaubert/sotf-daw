//! Render ordinary ITU downmix followed by an explicit separate remix stage.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use serde_json::json;
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, Plugin, ProcessContext};
use sotf_plugin_downmix::source_remix::{RemixParams, SourceMask, SourceRemix};
use sotf_plugin_downmix::{DownmixPlugin, DownmixPluginParams};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ModelFile {
    path: PathBuf,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    input: PathBuf,
    output: PathBuf,
    baseline_output: PathBuf,
    gain_trace_output: PathBuf,
    sample_rate: u32,
    frames: usize,
    callback_frames: usize,
    drain_capacity_frames: usize,
    downmix: DownmixPluginParams,
    remix: RemixParams,
    model: Option<ModelFile>,
}
fn write_pcm(path: &PathBuf, values: &[f32]) -> Result<(), Box<dyn std::error::Error>> {
    let mut out = BufWriter::new(OpenOptions::new().write(true).create_new(true).open(path)?);
    for value in values {
        out.write_all(&value.to_le_bytes())?;
    }
    out.flush()?;
    Ok(())
}
fn record(
    remix: &SourceRemix,
    previous: &mut u64,
    trace: &mut Vec<f32>,
) -> Result<(), Box<dyn std::error::Error>> {
    let current = remix.rendered_frames();
    if current == *previous {
        return Ok(());
    }
    if current != *previous + 1 {
        return Err("trace callback crossed more than one analysis boundary".into());
    }
    let (mid, side) = remix.spectral_gains();
    trace.extend_from_slice(mid);
    trace.extend_from_slice(side);
    *previous = current;
    Ok(())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let file = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-source-remix-render REQUEST.json")?;
    let mut raw = Vec::new();
    File::open(file)?.take(65_537).read_to_end(&mut raw)?;
    if raw.len() > 65_536 {
        return Err("request exceeds64KiB".into());
    }
    let r: Request = serde_json::from_slice(&raw)?;
    if !(44100..=192000).contains(&r.sample_rate)
        || r.frames == 0
        || r.frames > 1_000_000
        || !(1..=256).contains(&r.callback_frames)
        || !(1..=256).contains(&r.drain_capacity_frames)
        || r.downmix.input_channels != 6
        || r.downmix.input_layout.as_deref() != Some("5.1")
        || !r.downmix.itu_mode
        || r.downmix.phase_coherence
        || r.downmix.matrix_ltrt
    {
        return Err("this qualification renderer requires explicit matrix-only ITU5.1 and bounded callbacks".into());
    }
    let mut load_error = None;
    let model = if let Some(file) = &r.model {
        let loaded = (|| -> Result<SourceMask, String> {
            let mut bytes = Vec::new();
            File::open(&file.path)
                .map_err(|e| e.to_string())?
                .take(65_537)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            SourceMask::from_json(&bytes, &file.sha256)
        })();
        match loaded {
            Ok(m) => Some(m),
            Err(e) => {
                load_error = Some(e);
                None
            }
        }
    } else {
        None
    };
    let loaded = model.is_some();
    let mut downmix = DownmixPlugin::try_from_params(r.downmix.clone())?;
    downmix.initialize(f64::from(r.sample_rate))?;
    if downmix.latency_samples() != 0 {
        return Err("expected zero-latency matrix downmix".into());
    }
    let mut remix = SourceRemix::new(r.remix.clone(), model, f64::from(r.sample_rate))?;
    raw.clear();
    File::open(&r.input)?
        .take((r.frames * 6 * 4 + 1) as u64)
        .read_to_end(&mut raw)?;
    if raw.len() != r.frames * 6 * 4 {
        return Err("input PCM byte count mismatch".into());
    }
    let input: Vec<_> = raw
        .as_chunks::<4>()
        .0
        .iter()
        .copied()
        .map(f32::from_le_bytes)
        .collect();
    if input.iter().any(|v| !v.is_finite() || v.abs() > 16.0) {
        return Err("input must be finite within+/-16".into());
    }
    let mut baseline = Vec::with_capacity(r.frames * 2);
    let mut output = Vec::with_capacity((r.frames + 1024) * 2);
    let mut routed = vec![0.0; r.callback_frames * 2];
    let mut remixed = vec![0.0; r.callback_frames * 2];
    let mut trace = Vec::with_capacity((r.frames + 1024).div_ceil(256) * 514);
    let mut recorded = 0;
    for start in (0..r.frames).step_by(r.callback_frames) {
        let count = (r.frames - start).min(r.callback_frames);
        let context = ProcessContext::new(r.sample_rate, count);
        let mut done = Ok(0);
        assert_no_allocs_or_deallocs("ordinary ITU downmix", || {
            done = downmix.process(
                &input[start * 6..(start + count) * 6],
                &mut routed[..count * 2],
                &context,
            )
        });
        if done? != count {
            return Err("downmix frame mismatch".into());
        }
        let mut done = Ok(0);
        assert_no_allocs_or_deallocs("separate source remix", || {
            done = remix.process(&routed[..count * 2], &mut remixed[..count * 2], &context)
        });
        if done? != count {
            return Err("remix frame mismatch".into());
        }
        baseline.extend_from_slice(&routed[..count * 2]);
        output.extend_from_slice(&remixed[..count * 2]);
        record(&remix, &mut recorded, &mut trace)?;
    }
    let counts = remix.counters();
    let bound = remix
        .drain_call_bound()
        .ok_or("missing finite source-remix bound")?
        .get();
    let mut drain = vec![0.0; r.drain_capacity_frames * 2];
    let mut complete = false;
    let mut calls = 0;
    for _ in 0..bound {
        let mut result = None;
        assert_no_allocs_or_deallocs("source-remix finite drain", || {
            result = Some(remix.drain(
                &mut drain,
                &ProcessContext::new(r.sample_rate, r.drain_capacity_frames),
            ))
        });
        let result = result.unwrap()?;
        calls += 1;
        output.extend_from_slice(&drain[..result.frames * 2]);
        record(&remix, &mut recorded, &mut trace)?;
        if result.complete {
            complete = true;
            break;
        }
    }
    if !complete
        || counts != remix.counters()
        || output.iter().any(|v| !v.is_finite() || v.abs() > 256.0)
    {
        return Err("incomplete, adaptive or invalid source-remix drain".into());
    }
    write_pcm(&r.baseline_output, &baseline)?;
    write_pcm(&r.output, &output)?;
    write_pcm(&r.gain_trace_output, &trace)?;
    println!(
        "{}",
        json!({"status":"ok","pipeline_mode":"ordinary_downmix_then_separate_source_remix","sample_rate":r.sample_rate,
        "input_channels":6,"output_channels":2,"input_frames":r.frames,"output_frames":output.len()/2,"callback_frames":r.callback_frames,
        "drain_frames":output.len()/2-r.frames,"drain_calls":calls,"drain_call_bound":bound,"drain_capacity_frames":r.drain_capacity_frames,
        "downmix_parameters":r.downmix,"remix_parameters":r.remix,"candidate_loaded":loaded,"load_error":load_error,
        "counts":counts,"trace_frames":recorded,"trace_bins":257,"analysis_window":remix.analysis_window(),
        "latency_samples":remix.latency_samples(),"downmix_latency_samples":0,"added_model_lookahead_samples":0,
        "deployment_admitted":false,"scope":"Explicit separate unadmitted stage after unchanged ITU downmix. M/S conditional gains are traced for independent stem and waveform reconstruction. Spatial foreground is not semantic speech identity. No normalization or limiter."})
    );
    Ok(())
}
