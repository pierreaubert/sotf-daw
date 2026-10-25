//! Render the actual native MVDR path with optional unadmitted spatial masks.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use serde_json::json;
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, Plugin, ProcessContext};
use sotf_plugin_beamformer::noise_mask::NoiseMask;
use sotf_plugin_beamformer::{BeamformerPlugin, BeamformerPluginParams};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;
use std::time::Instant;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;
const MAX_REQUEST: usize = 65_536;
const MAX_SAMPLES: usize = 8_000_000;

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
    sample_rate: u32,
    frames: usize,
    callback_frames: usize,
    appended_zero_frames: usize,
    parameters: BeamformerPluginParams,
    model: Option<ModelFile>,
    enabled: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-beamformer-mask-render REQUEST.json")?;
    let mut bytes = Vec::new();
    File::open(path)?
        .take((MAX_REQUEST + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_REQUEST {
        return Err("request exceeds64KiB".into());
    }
    let r: Request = serde_json::from_slice(&bytes)?;
    let m = r.parameters.num_mics;
    let frames = r
        .frames
        .checked_add(r.appended_zero_frames)
        .ok_or("frame overflow")?;
    if !(16000..=96000).contains(&r.sample_rate)
        || r.parameters.beamformer_type != 0
        || !(2..=8).contains(&m)
        || r.frames == 0
        || frames.checked_mul(m).is_none_or(|n| n > MAX_SAMPLES)
        || r.appended_zero_frames > 65536
        || !(1..=4096).contains(&r.callback_frames)
    {
        return Err("unsupported bounded MVDR geometry".into());
    }
    let mut load_error = None;
    let model = if let Some(file) = &r.model {
        let result = (|| -> Result<NoiseMask, String> {
            let mut raw = Vec::new();
            File::open(&file.path)
                .map_err(|e| e.to_string())?
                .take(65_537)
                .read_to_end(&mut raw)
                .map_err(|e| e.to_string())?;
            NoiseMask::from_json(&raw, &file.sha256)
        })();
        match result {
            Ok(model) => Some(model),
            Err(e) => {
                load_error = Some(e);
                None
            }
        }
    } else {
        None
    };
    let loaded = model.is_some();
    let mut plugin = BeamformerPlugin::from_params(r.sample_rate, r.parameters.clone())?;
    plugin.initialize(f64::from(r.sample_rate))?;
    plugin.install_qa_noise_mask(model)?;
    plugin.set_qa_noise_mask_enabled(r.enabled)?;
    assert_no_allocs_or_deallocs("mask renderer fresh reset", || plugin.reset());
    let latency = plugin.latency_samples();
    if latency != 512 {
        return Err("native MVDR latency changed".into());
    }
    let mut raw = Vec::new();
    File::open(&r.input)?
        .take((r.frames * m * 4 + 1) as u64)
        .read_to_end(&mut raw)?;
    if raw.len() != r.frames * m * 4 {
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
    let mut block = vec![0.0; r.callback_frames * m];
    let mut mono = vec![0.0; r.callback_frames];
    let mut output = Vec::with_capacity(frames);
    let start = Instant::now();
    for position in (0..frames).step_by(r.callback_frames) {
        let count = (frames - position).min(r.callback_frames);
        block[..count * m].fill(0.0);
        let real = r.frames.saturating_sub(position).min(count) * m;
        if real > 0 {
            block[..real].copy_from_slice(&input[position * m..position * m + real]);
        }
        let context =
            ProcessContext::new(r.sample_rate, count).with_sample_position(position as u64);
        let mut done = Ok(0);
        assert_no_allocs_or_deallocs("mask renderer native process", || {
            done = plugin.process(&block[..count * m], &mut mono[..count], &context);
        });
        if done? != count {
            return Err("native output count mismatch".into());
        }
        output.extend_from_slice(&mono[..count]);
    }
    let seconds = start.elapsed().as_secs_f64();
    if output.iter().any(|v| !v.is_finite() || v.abs() > 128.0) {
        return Err("native output rail violation".into());
    }
    let peak = output.iter().fold(0.0_f32, |a, v| a.max(v.abs()));
    let mut writer = BufWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&r.output)?,
    );
    for sample in output {
        writer.write_all(&sample.to_le_bytes())?;
    }
    writer.flush()?;
    println!(
        "{}",
        json!({"status":"ok","sample_rate":r.sample_rate,"input_channels":m,"output_channels":1,
        "input_frames":r.frames,"output_frames":frames,"callback_frames":r.callback_frames,
        "appended_zero_frames":r.appended_zero_frames,"effective_parameters":r.parameters,"candidate_loaded":loaded,
        "load_error":load_error,"enabled":r.enabled,"counts":plugin.qa_noise_mask_counters(),"latency_samples":latency,
        "added_model_lookahead_samples":0,"qa_process_seconds":seconds,"sample_peak":peak,"deployment_admitted":false,
        "scope":"Actual native MVDR stream with independently running classical fallback. Generated unadmitted mask; no normalization, gain/delay fitting, extra output limiter or finite-zero-tail claim."})
    );
    Ok(())
}
