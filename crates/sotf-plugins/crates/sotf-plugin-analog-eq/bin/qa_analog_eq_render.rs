//! Render actual Analog EQ for offline calibrated hardware-response fitting.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use serde_json::{Value, json};
use sotf_host::plugin_params::PluginParamDef;
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_analog_common::MODEL_NAMES;
use sotf_plugin_analog_eq::{AnalogEqPlugin, AnalogEqPluginParams, params::PARAMS};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;
// Bound input plus explicit observation zeros; recursive support is not finite.
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
    appended_zero_frames: usize,
    parameters: Value,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-analog-eq-render REQUEST.json")?;
    let mut raw = Vec::new();
    File::open(path)?
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut raw)?;
    if raw.len() > MAX_REQUEST_BYTES {
        return Err("request exceeds64KiB".into());
    }
    let r: Request = serde_json::from_slice(&raw)?;
    let frames = r
        .frames
        .checked_add(r.appended_zero_frames)
        .ok_or("frame overflow")?;
    let samples = frames.checked_mul(r.channels).ok_or("sample overflow")?;
    if !(44100..=192000).contains(&r.sample_rate)
        || !(1..=32).contains(&r.channels)
        || r.frames == 0
        || samples > MAX_SAMPLES
        || !(1..=4096).contains(&r.callback_frames)
        || r.appended_zero_frames > 65536
    {
        return Err("unsupported bounded native Analog EQ geometry".into());
    }
    let object = r
        .parameters
        .as_object()
        .ok_or("supply complete native Analog EQ parameters")?;
    if object.len() != PARAMS.len() || PARAMS.iter().any(|p| !object.contains_key(p.engine_key)) {
        return Err("supply exact complete native Analog EQ schema".into());
    }
    let mut params: AnalogEqPluginParams = serde_json::from_value(r.parameters)?;
    if !MODEL_NAMES.contains(&params.analog_model.as_str()) {
        return Err("use canonical native model names".into());
    }
    for (index, spec) in PARAMS.iter().enumerate() {
        let value = params
            .param_value(index)
            .ok_or("missing native parameter value")?;
        if !value.is_finite() || value < spec.min_f64() || value > spec.max_f64() {
            return Err(format!(
                "parameter outside native registry range: {}",
                spec.engine_key
            )
            .into());
        }
    }
    // The biquad controls use f64; the shared analog stage stores these in f32.
    params.analog_drive = f64::from(params.analog_drive as f32);
    params.analog_color = f64::from(params.analog_color as f32);
    params.analog_character = f64::from(params.analog_character as f32);
    params.analog_trim = f64::from(params.analog_trim as f32);
    let mut plugin = AnalogEqPlugin::from_params(r.channels, params.clone())?;
    plugin.initialize(r.sample_rate.into())?;
    for (id, value) in plugin.current_values() {
        plugin.parametric_validate_parameter(&id, &value)?;
    }
    assert_no_allocs_or_deallocs("Analog EQ fresh reset", || plugin.reset());
    if plugin.latency_samples() != 0 {
        return Err("unexpected native latency".into());
    }
    let count = r.frames * r.channels;
    let mut bytes = Vec::new();
    File::open(&r.input)?
        .take((count * 4 + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() != count * 4 {
        return Err("PCM byte count mismatch".into());
    }
    let input: Vec<_> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .copied()
        .map(f32::from_le_bytes)
        .collect();
    if input.iter().any(|v| !v.is_finite() || v.abs() > 16.0) {
        return Err("PCM must be finite within+/-16".into());
    }
    let mut block = vec![0.0; r.callback_frames * r.channels];
    let mut output = Vec::with_capacity(samples);
    for position in (0..frames).step_by(r.callback_frames) {
        let count = (frames - position).min(r.callback_frames);
        let used = count * r.channels;
        block[..used].fill(0.0);
        let real = r.frames.saturating_sub(position).min(count) * r.channels;
        if real > 0 {
            block[..real]
                .copy_from_slice(&input[position * r.channels..position * r.channels + real]);
        }
        let context =
            ProcessContext::new(r.sample_rate, count).with_sample_position(position as u64);
        let mut result = Ok(0);
        assert_no_allocs_or_deallocs("Analog EQ native process", || {
            result = plugin.process_in_place(&mut block[..used], &context)
        });
        if result? != count {
            return Err("native frame count mismatch".into());
        }
        output.extend_from_slice(&block[..used]);
    }
    if output.iter().any(|v| !v.is_finite() || v.abs() > 128.0) {
        return Err("native output rail violation".into());
    }
    let mut file = BufWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&r.output)?,
    );
    for sample in output {
        file.write_all(&sample.to_le_bytes())?;
    }
    file.flush()?;
    println!(
        "{}",
        json!({"status":"ok","sample_rate":r.sample_rate,"channels":r.channels,"input_frames":r.frames,
        "output_frames":frames,"callback_frames":r.callback_frames,"appended_zero_frames":r.appended_zero_frames,
        "effective_params":params,"latency_samples":0,"tail":"unknown_recursive",
        "scope":"Actual four-biquad then shared-color Analog EQ. Fresh static controls, no normalization, gain/delay fit, extra limiting or automatic parameter decisions. Explicit zeros do not establish finite recursive support."})
    );
    Ok(())
}
