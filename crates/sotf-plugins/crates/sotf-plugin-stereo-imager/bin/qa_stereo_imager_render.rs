//! Render native Stereo Imager audio for offline learned-setting qualification.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use serde_json::{Value, json};
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_stereo_imager::{StereoImagerPlugin, StereoImagerPluginParams};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;
// Bound offline programme audio and explicit observation zeros, not IIR tail length.
const MAX_SAMPLES: usize = 4_000_000;
const MAX_ZERO_FRAMES: usize = 65_536;
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
    appended_zero_frames: usize,
    params: Value,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-stereo-imager-render REQUEST.json")?;
    let mut raw = Vec::new();
    File::open(path)?
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut raw)?;
    if raw.len() > MAX_REQUEST_BYTES {
        return Err("request exceeds64KiB".into());
    }
    let request: Request = serde_json::from_slice(&raw)?;
    let samples = request
        .frames
        .checked_mul(request.channels)
        .ok_or("sample count overflow")?;
    if request.channels != 2
        || !(44100..=192000).contains(&request.sample_rate)
        || samples == 0
        || samples > MAX_SAMPLES
        || !(1..=4096).contains(&request.callback_frames)
        || request.appended_zero_frames > MAX_ZERO_FRAMES
    {
        return Err("unsupported bounded Stereo Imager geometry".into());
    }
    let object = request
        .params
        .as_object()
        .ok_or("supply explicit native parameters")?;
    let keys = [
        "width",
        "low_mid_freq",
        "mid_high_freq",
        "low_width",
        "mid_width",
        "high_width",
        "mono_bass",
        "mix",
    ];
    if object.len() != keys.len() || keys.iter().any(|key| !object.contains_key(*key)) {
        return Err("supply the exact complete native parameter schema".into());
    }
    let params: StereoImagerPluginParams = serde_json::from_value(request.params)?;
    let mut plugin = StereoImagerPlugin::try_from_params(2, params.clone())?;
    plugin.initialize(request.sample_rate.into())?;
    assert_no_allocs_or_deallocs("Stereo Imager fresh reset", || plugin.reset());
    if plugin.latency_samples() != 0 {
        return Err("expected native zero latency".into());
    }
    let effective = plugin.current_values();
    for (key, value) in &effective {
        plugin.parametric_validate_parameter(key, value)?;
    }
    let mut bytes = Vec::new();
    File::open(&request.input)?
        .take((samples * 4 + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() != samples * 4 {
        return Err("PCM byte length mismatch".into());
    }
    let input: Vec<_> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .copied()
        .map(f32::from_le_bytes)
        .collect();
    if input.iter().any(|v| !v.is_finite() || v.abs() > 32.0) {
        return Err("PCM must be finite within+/-32".into());
    }
    let frames = request.frames + request.appended_zero_frames;
    let mut output = Vec::with_capacity(frames * 2);
    let mut block = vec![0.0; request.callback_frames * 2];
    for position in (0..frames).step_by(request.callback_frames) {
        let count = (frames - position).min(request.callback_frames);
        let used = count * 2;
        block[..used].fill(0.0);
        let real = request.frames.saturating_sub(position).min(count) * 2;
        if real > 0 {
            block[..real].copy_from_slice(&input[position * 2..position * 2 + real]);
        }
        let context =
            ProcessContext::new(request.sample_rate, count).with_sample_position(position as u64);
        let mut result = Ok(0);
        assert_no_allocs_or_deallocs("Stereo Imager native render", || {
            result = plugin.process_in_place(&mut block[..used], &context)
        });
        if result? != count {
            return Err("native frame count mismatch".into());
        }
        output.extend_from_slice(&block[..used]);
    }
    if output.iter().any(|v| !v.is_finite()) {
        return Err("non-finite native output".into());
    }
    let mut file = BufWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&request.output)?,
    );
    for sample in output {
        file.write_all(&sample.to_le_bytes())?;
    }
    file.flush()?;
    println!(
        "{}",
        json!({"status":"ok","sample_rate":request.sample_rate,"channels":2,
        "input_frames":request.frames,"output_frames":frames,"appended_zero_frames":request.appended_zero_frames,
        "callback_frames":request.callback_frames,"latency_samples":0,"effective_params":params,
        "scope":"Native fresh static controls; explicit zeros are processed input, not a finite IIR drain claim. No learned inference, gain normalization, limiting or automatic setting changes."})
    );
    Ok(())
}
