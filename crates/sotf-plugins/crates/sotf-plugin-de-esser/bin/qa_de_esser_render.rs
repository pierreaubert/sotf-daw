//! Export native de-esser audio for optional learned sidechain experiments.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use serde_json::json;
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_de_esser::{DeEsserPlugin, DeEsserPluginParams};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;
// Offline limits bound both program and optional key allocations.
const MAX_SAMPLES: usize = 4_000_000;
const MAX_REQUEST_BYTES: usize = 65_536;
const MAX_TAIL_FRAMES: usize = 65_536;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    input: PathBuf,
    output: PathBuf,
    key: Option<PathBuf>,
    sample_rate: u32,
    channels: usize,
    frames: usize,
    callback_frames: usize,
    params: DeEsserPluginParams,
}

fn read_audio(path: &Path, samples: usize) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let mut bytes = Vec::new();
    File::open(path)?
        .take((samples * 4 + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() != samples * 4 {
        return Err("audio must contain exactly frames*channels float32-le samples".into());
    }
    let audio: Vec<_> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .copied()
        .map(f32::from_le_bytes)
        .collect();
    if audio.iter().any(|v| !v.is_finite() || v.abs() > 32.0) {
        return Err("audio samples must be finite within +/-32".into());
    }
    Ok(audio)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-de-esser-render REQUEST.json")?;
    let mut bytes = Vec::new();
    File::open(path)?
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err("request exceeds size limit".into());
    }
    let request: Request = serde_json::from_slice(&bytes)?;
    let samples = request
        .frames
        .checked_mul(request.channels)
        .ok_or("sample count overflow")?;
    if !(44_100..=192_000).contains(&request.sample_rate)
        || !(1..=8).contains(&request.channels)
        || samples == 0
        || samples > MAX_SAMPLES
        || !(1..=4096).contains(&request.callback_frames)
        || request.params.sidechain_external != request.key.is_some()
    {
        return Err("unsupported geometry or inconsistent external key request".into());
    }
    let input = read_audio(&request.input, samples)?;
    let key = request
        .key
        .as_ref()
        .map(|path| read_audio(path, samples))
        .transpose()?;
    let mut plugin = DeEsserPlugin::try_from_params_at_sample_rate(
        request.channels,
        request.params.clone(),
        request.sample_rate,
    )?;
    plugin.initialize(f64::from(request.sample_rate))?;
    let latency = plugin.latency_samples();
    let stride = plugin.input_channels();
    let mut block = vec![0.0; request.callback_frames * stride];
    let mut output = Vec::with_capacity((request.frames + latency + 512) * request.channels);
    for position in (0..request.frames).step_by(request.callback_frames) {
        let frames = (request.frames - position).min(request.callback_frames);
        for frame in 0..frames {
            let source = (position + frame) * request.channels;
            let destination = frame * stride;
            block[destination..destination + request.channels]
                .copy_from_slice(&input[source..source + request.channels]);
            if let Some(key) = &key {
                block[destination + request.channels..destination + stride]
                    .copy_from_slice(&key[source..source + request.channels]);
            }
        }
        let context =
            ProcessContext::new(request.sample_rate, frames).with_sample_position(position as u64);
        let mut result = Ok(0);
        assert_no_allocs_or_deallocs("de-esser render process", || {
            result = plugin.process_in_place(&mut block[..frames * stride], &context);
        });
        if result? != frames {
            return Err("native process changed frame count".into());
        }
        for frame in 0..frames {
            let offset = frame * stride;
            output.extend_from_slice(&block[offset..offset + request.channels]);
            if let Some(key) = &key {
                for channel in 0..request.channels {
                    let expected = key[(position + frame) * request.channels + channel];
                    let actual = block[offset + request.channels + channel];
                    if actual != expected && !(actual == 0.0 && expected.abs() < f32::MIN_POSITIVE)
                    {
                        return Err("native processor changed a normal external key sample".into());
                    }
                }
            }
        }
    }
    let capacity = plugin.drain_output_frames_max();
    let call_bound = plugin
        .drain_call_bound()
        .ok_or("missing native drain bound")?
        .get();
    if capacity > MAX_TAIL_FRAMES || call_bound > MAX_TAIL_FRAMES as u64 {
        return Err("native drain exceeds QA bounds".into());
    }
    let mut tail = vec![0.0; capacity * request.channels];
    let mut complete = false;
    let mut drained = 0;
    for _ in 0..call_bound {
        let mut result = None;
        assert_no_allocs_or_deallocs("de-esser render drain", || {
            result = Some(plugin.drain(&mut tail, &ProcessContext::new(request.sample_rate, 0)));
        });
        let result = result.expect("drain closure ran")?;
        if result.frames > capacity {
            return Err("native drain exceeded capacity".into());
        }
        drained += result.frames;
        if drained > MAX_TAIL_FRAMES {
            return Err("native tail exceeds QA bound".into());
        }
        output.extend_from_slice(&tail[..result.frames * request.channels]);
        if result.complete {
            complete = true;
            break;
        }
    }
    if !complete
        || drained < latency
        || output.len() != (request.frames + drained) * request.channels
        || output.iter().any(|v| !v.is_finite())
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
        json!({"status":"ok","sample_rate":request.sample_rate,"channels":request.channels,
        "input_frames":request.frames,"output_frames":request.frames+drained,"latency_samples":latency,
        "drained_frames":drained,"callback_frames":request.callback_frames,"requested_params":request.params,
        "scope":"Actual native de-esser; external key retained; process/drain heap guards; public drain preserves delay/FIR support and truncates recursive minimum-phase ringing"})
    );
    Ok(())
}
