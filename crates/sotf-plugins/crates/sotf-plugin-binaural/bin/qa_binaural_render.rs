//! Export fixed-pose native binaural audio for offline HRTF selection.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use serde_json::json;
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, Plugin, ProcessContext};
use sotf_plugin_binaural::config::SUPPORTED_INPUT_CHANNELS;
use sotf_plugin_binaural::{BinauralDecoderParams, BinauralDecoderPlugin};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;
// Offline limits bound program allocations and finite native output support.
const MAX_SAMPLES: usize = 4_000_000;
const MAX_REQUEST_BYTES: usize = 65_536;
const MAX_TAIL_FRAMES: usize = 65_536;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    input: PathBuf,
    output: PathBuf,
    hrtf: PathBuf,
    sample_rate: u32,
    channels: usize,
    frames: usize,
    callback_frames: usize,
    fft_size: usize,
    diffuse_field_eq: bool,
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
        .ok_or("usage: qa-binaural-render REQUEST.json")?;
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
        || !SUPPORTED_INPUT_CHANNELS.contains(&request.channels)
        || samples == 0
        || samples > MAX_SAMPLES
        || !(1..=4096).contains(&request.callback_frames)
        || !(512..=8192).contains(&request.fft_size)
        || !request.fft_size.is_power_of_two()
    {
        return Err("unsupported offline binaural geometry".into());
    }
    // File size is an input limit, not a total-memory or decompression bound.
    let hrtf_metadata = std::fs::metadata(&request.hrtf)?;
    if !hrtf_metadata.is_file() || hrtf_metadata.len() == 0 || hrtf_metadata.len() > 16_777_216 {
        return Err("use a nonempty SOFA file of at most 16 MiB".into());
    }
    let input = read_audio(&request.input, samples)?;
    let hrtf = request.hrtf.to_str().ok_or("SOFA path must be UTF-8")?;
    let params: BinauralDecoderParams = serde_json::from_value(json!({
        "input_channels":request.channels, "fft_size":request.fft_size,
        "hrtf_file":hrtf, "externalization":1.0, "near_field_strength":0.0,
        "diffuse_field_eq":request.diffuse_field_eq, "lfe_crossover":120.0,
        "lfe_distance":2.0, "lfe_level":0.0, "room_model":{"max_order":0},
        "hrtf_database_dir":"", "srir_file":"", "crossfade_mode":0,
        "late_reverb_enabled":false
    }))?;
    let mut plugin = BinauralDecoderPlugin::try_from_params(params.clone())?;
    // A bad/missing SOFA returns an error; no fallback render may be ranked.
    plugin.initialize(f64::from(request.sample_rate))?;
    let latency = plugin.latency_samples();
    let mut block = vec![0.0; request.callback_frames * 2];
    let mut output = Vec::with_capacity((request.frames + MAX_TAIL_FRAMES) * 2);
    for position in (0..request.frames).step_by(request.callback_frames) {
        let frames = (request.frames - position).min(request.callback_frames);
        let context =
            ProcessContext::new(request.sample_rate, frames).with_sample_position(position as u64);
        let mut result = Ok(0);
        assert_no_allocs_or_deallocs("binaural selection render process", || {
            result = plugin.process(
                &input[position * request.channels..(position + frames) * request.channels],
                &mut block[..frames * 2],
                &context,
            );
        });
        if result? != frames {
            return Err("native process changed frame count".into());
        }
        output.extend_from_slice(&block[..frames * 2]);
    }
    let capacity = plugin.drain_output_frames_max();
    let call_bound = plugin
        .drain_call_bound()
        .ok_or("missing native drain bound")?
        .get();
    if capacity > MAX_TAIL_FRAMES || call_bound > MAX_TAIL_FRAMES as u64 {
        return Err("native drain exceeds QA bounds".into());
    }
    let mut tail = vec![0.0; capacity * 2];
    let mut complete = false;
    let mut drained = 0;
    for _ in 0..call_bound {
        let mut result = None;
        assert_no_allocs_or_deallocs("binaural selection render drain", || {
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
        output.extend_from_slice(&tail[..result.frames * 2]);
        if result.complete {
            complete = true;
            break;
        }
    }
    if !complete
        || drained < latency
        || output.len() != (request.frames + drained) * 2
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
        json!({"status":"ok","sample_rate":request.sample_rate,"input_channels":request.channels,
        "output_channels":2,"input_frames":request.frames,"output_frames":request.frames+drained,
        "latency_samples":latency,"drained_frames":drained,"callback_frames":request.callback_frames,
        "requested_params":params,"hrtf_file_bytes":hrtf_metadata.len(),
        "scope":"Fixed zero head pose, explicit SOFA, native normalization, room/late-reverb off; full public finite tail; no preference, listener or moving-head acceptance claim"})
    );
    Ok(())
}
