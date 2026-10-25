//! Export bounded native Declick renders with exact public drain alignment.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use serde_json::json;
use sotf_host::{CountingAlloc, ParametricInPlacePlugin, ProcessContext, assert_no_allocs};
use sotf_plugin_declick::{DeclickPlugin, DeclickPluginParams};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

// Offline QA limits bound memory and accidental oversized training requests.
const MAX_SAMPLES: usize = 4_000_000;
const MAX_REQUEST_BYTES: usize = 65_536;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    input: PathBuf,
    output: PathBuf,
    sample_rate: u32,
    channels: usize,
    frames: usize,
    callback_frames: usize,
    params: serde_json::Value,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-declick-render REQUEST.json")?;
    let mut bytes = Vec::new();
    File::open(path)?
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err("request exceeds size limit".into());
    }
    let request: Request = serde_json::from_slice(&bytes)?;
    let allowed = [
        "enabled",
        "sensitivity",
        "link_channels",
        "mode",
        "bands",
        "crossover_hz",
        "frequency_skew",
        "repair_width",
        "audition_residual",
    ];
    if !request
        .params
        .as_object()
        .is_some_and(|object| object.keys().all(|key| allowed.contains(&key.as_str())))
    {
        return Err("params must be an object with known native fields".into());
    }
    let params: DeclickPluginParams = serde_json::from_value(request.params.clone())?;
    if !params.sensitivity.is_finite()
        || !(1.0..=100.0).contains(&params.sensitivity)
        || !params.crossover_hz.is_finite()
        || !(80.0..=12_000.0).contains(&params.crossover_hz)
        || !params.frequency_skew.is_finite()
        || !(-1.0..=1.0).contains(&params.frequency_skew)
        || params.repair_width > 8
        || params.mode > 1
        || params.bands > 2
    {
        return Err("QA parameters must not require native clamping".into());
    }

    let samples = request
        .frames
        .checked_mul(request.channels)
        .ok_or("sample count overflow")?;
    if !(8_000..=192_000).contains(&request.sample_rate)
        || !(1..=8).contains(&request.channels)
        || samples == 0
        || samples > MAX_SAMPLES
        || !(1..=4_096).contains(&request.callback_frames)
        || params.audition_residual
    {
        return Err("unsupported render geometry or residual audition".into());
    }
    let expected_bytes = samples * 4;
    bytes.clear();
    File::open(&request.input)?
        .take((expected_bytes + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() != expected_bytes {
        return Err("input must contain exactly frames*channels float32-le samples".into());
    }
    let input: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .copied()
        .map(f32::from_le_bytes)
        .collect();
    if input
        .iter()
        .any(|sample| !sample.is_finite() || sample.abs() > 32.0)
    {
        return Err("input samples must be finite and within +/-32".into());
    }
    let mut plugin =
        DeclickPlugin::from_params(request.channels, request.sample_rate, params.clone())?;
    plugin.initialize(f64::from(request.sample_rate))?;
    let latency = plugin.latency_samples();
    let mut output = Vec::with_capacity((request.frames + latency) * request.channels);
    let mut block = vec![0.0; request.callback_frames * request.channels];
    for (index, source) in input.chunks(block.len()).enumerate() {
        block[..source.len()].copy_from_slice(source);
        let frames = source.len() / request.channels;
        let context = ProcessContext::new(request.sample_rate, frames)
            .with_sample_position((index * request.callback_frames) as u64);
        let mut result = Ok(0);
        assert_no_allocs("Declick export process", || {
            result = plugin.process_in_place(&mut block[..source.len()], &context);
        });
        if result? != frames {
            return Err("native process changed frame count".into());
        }
        output.extend_from_slice(&block[..source.len()]);
    }
    let capacity = plugin.drain_output_frames_max();
    let mut tail = vec![0.0; capacity * request.channels];
    let mut complete = false;
    let mut drained = 0;
    // The public Declick drain is bounded by its maximum 16-sample latency.
    // Four calls expose a broken completion contract instead of spinning.
    for _ in 0..4 {
        let context = ProcessContext::new(request.sample_rate, capacity)
            .with_sample_position((request.frames + drained) as u64);
        let mut result = None;
        assert_no_allocs("Declick export drain", || {
            result = Some(plugin.drain(&mut tail, &context));
        });
        let result = result.expect("drain closure ran")?;
        if result.frames > capacity {
            return Err("native drain exceeded capacity".into());
        }
        drained += result.frames;
        output.extend_from_slice(&tail[..result.frames * request.channels]);
        if result.complete {
            complete = true;
            break;
        }
    }
    if !complete
        || drained != latency
        || output.len() != (request.frames + latency) * request.channels
        || output.iter().any(|sample| !sample.is_finite())
    {
        return Err("invalid native output or drain alignment".into());
    }
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&request.output)?;
    let mut writer = BufWriter::new(file);
    for sample in output {
        writer.write_all(&sample.to_le_bytes())?;
    }
    writer.flush()?;
    println!(
        "{}",
        json!({"status":"ok", "sample_rate":request.sample_rate,
               "channels":request.channels,"input_frames":request.frames,
               "output_frames":request.frames + latency,"latency_samples":latency,
               "drained_frames":drained,"callback_frames":request.callback_frames,
               "requested_params":params,
               "scope":"native public Declick, untrimmed latency prefix and exact drain; no learned processing"})
    );
    Ok(())
}
