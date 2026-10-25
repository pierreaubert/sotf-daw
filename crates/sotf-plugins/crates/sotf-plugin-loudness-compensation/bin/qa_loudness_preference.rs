//! Export the actual stationary manual-mode impulse for preference qualification.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use serde_json::json;
use sotf_host::{CountingAlloc, ParametricInPlacePlugin, ProcessContext, assert_no_allocs};
use sotf_plugin_loudness_compensation::{
    LoudnessCompensationPlugin, LoudnessCompensationPluginParams,
};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

// At192kHz this covers over0.68s, far beyond the lowest20Hz shelf time scale.
// Consumers must still check the exported tail instead of assuming truncation.
const FRAMES: usize = 131_072;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    sample_rate: u32,
    callback_frames: usize,
    output: PathBuf,
    params: LoudnessCompensationPluginParams,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-loudness-preference REQUEST.json")?;
    let mut bytes = Vec::new();
    File::open(path)?.take(65_537).read_to_end(&mut bytes)?;
    if bytes.len() > 65_536 {
        return Err("request exceeds QA size limit".into());
    }
    let request: Request = serde_json::from_slice(&bytes)?;
    let p = &request.params;
    if !(44_100..=192_000).contains(&request.sample_rate)
        || !(1..=4_096).contains(&request.callback_frames)
        || p.mode != 0
        || !p.mid_enabled
        || !p.channel_params.is_empty()
        || p.auto_gain_enabled
        || p.auto_gain_position != "disabled"
        || !p.headroom_normalized
        || p.auto_calibrated
        || [p.low_gain, p.mid_gain, p.high_gain]
            .iter()
            .any(|x| !x.is_finite() || x.abs() > 6.0)
    {
        return Err("unsupported bounded manual preference profile".into());
    }
    let mut plugin = LoudnessCompensationPlugin::from_params(2, p.clone())?;
    plugin.initialize(f64::from(request.sample_rate))?;
    let latency = plugin.latency_samples();
    let mut buffer = vec![0.0_f32; request.callback_frames * 2];
    let mut output = Vec::with_capacity(FRAMES * 2);
    let mut position = 0;
    while position < FRAMES {
        let frames = (FRAMES - position).min(request.callback_frames);
        buffer[..frames * 2].fill(0.0);
        if position == 0 {
            buffer[0] = 1.0;
            buffer[1] = 1.0;
        }
        let context =
            ProcessContext::new(request.sample_rate, frames).with_sample_position(position as u64);
        let mut result = Ok(0);
        assert_no_allocs("manual loudness preference impulse", || {
            result = plugin.process_in_place(&mut buffer[..frames * 2], &context);
        });
        if result? != frames || buffer[..frames * 2].iter().any(|x| !x.is_finite()) {
            return Err("invalid native preference impulse".into());
        }
        output.extend_from_slice(&buffer[..frames * 2]);
        position += frames;
    }
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(request.output)?;
    let mut writer = BufWriter::new(file);
    for sample in output {
        writer.write_all(&sample.to_le_bytes())?;
    }
    writer.flush()?;
    println!(
        "{}",
        json!({"status":"ok","sample_rate":request.sample_rate,
        "channels":2,"frames":FRAMES,"callback_frames":request.callback_frames,
        "latency_samples":latency,"scope":"native stationary manual curve, peak normalization, AutoGain off; truncated impulse tail must be checked"})
    );
    Ok(())
}
