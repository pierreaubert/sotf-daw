//! Render the actual denoiser with optional unadmitted musical gains and full drain.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use serde_json::json;
use sotf_host::plugin::PluginDrainResult;
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_denoiser::music_mask::MusicMask;
use sotf_plugin_denoiser::{DenoiserPlugin, DenoiserPluginParams};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;
use std::time::Instant;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;
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
    channels: usize,
    frames: usize,
    callback_frames: usize,
    drain_capacity_frames: usize,
    parameters: DenoiserPluginParams,
    model: Option<ModelFile>,
    enabled: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-music-mask-render REQUEST.json")?;
    let mut raw = Vec::new();
    File::open(path)?.take(65_537).read_to_end(&mut raw)?;
    if raw.len() > 65_536 {
        return Err("request exceeds64KiB".into());
    }
    let r: Request = serde_json::from_slice(&raw)?;
    if !(44100..=192000).contains(&r.sample_rate)
        || !(1..=8).contains(&r.channels)
        || r.frames == 0
        || r.frames
            .checked_mul(r.channels)
            .is_none_or(|v| v > MAX_SAMPLES)
        || !(1..=512).contains(&r.callback_frames)
        || !(1..=2048).contains(&r.drain_capacity_frames)
    {
        return Err("unsupported bounded music-render geometry".into());
    }
    let mut load_error = None;
    let model = if let Some(file) = &r.model {
        let loaded = (|| -> Result<MusicMask, String> {
            let mut bytes = Vec::new();
            File::open(&file.path)
                .map_err(|e| e.to_string())?
                .take(65_537)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            MusicMask::from_json(&bytes, &file.sha256)
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
    let mut plugin = DenoiserPlugin::try_from_params(r.channels, r.parameters.clone())?;
    plugin.initialize(f64::from(r.sample_rate))?;
    plugin.install_qa_music_mask(model)?;
    plugin.set_qa_music_mask_enabled(r.enabled)?;
    assert_no_allocs_or_deallocs("music renderer reset", || plugin.reset());
    let latency = plugin.latency_samples();
    if latency != if r.parameters.low_latency { 512 } else { 2048 } {
        return Err("native denoiser latency mismatch".into());
    }
    let samples = r.frames * r.channels;
    raw.clear();
    File::open(&r.input)?
        .take((samples * 4 + 1) as u64)
        .read_to_end(&mut raw)?;
    if raw.len() != samples * 4 {
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
    let mut block = vec![0.0; r.callback_frames * r.channels];
    let mut output = Vec::with_capacity(samples + (2 * latency + 1) * r.channels);
    let start = Instant::now();
    for position in (0..r.frames).step_by(r.callback_frames) {
        let count = (r.frames - position).min(r.callback_frames);
        let used = count * r.channels;
        block[..used].copy_from_slice(&input[position * r.channels..position * r.channels + used]);
        let mut done = Ok(0);
        assert_no_allocs_or_deallocs("music renderer process", || {
            done = plugin.process_in_place(
                &mut block[..used],
                &ProcessContext::new(r.sample_rate, count),
            )
        });
        if done? != count {
            return Err("native source frame count mismatch".into());
        }
        output.extend_from_slice(&block[..used]);
    }
    let source_counts = plugin.qa_music_mask_counters();
    let refill_bound = plugin
        .drain_call_bound()
        .ok_or("missing finite native drain bound")?
        .get();
    let bound = refill_bound
        * plugin
            .drain_output_frames_max()
            .div_ceil(r.drain_capacity_frames) as u64;
    if bound > 8192 {
        return Err("unexpected excessive native drain work".into());
    }
    let mut drain = vec![0.0; r.drain_capacity_frames * r.channels];
    let mut completed = false;
    let mut calls = 0;
    for _ in 0..bound {
        let mut result = Ok(PluginDrainResult::COMPLETE);
        assert_no_allocs_or_deallocs("music renderer finite drain", || {
            result = plugin.drain(
                &mut drain,
                &ProcessContext::new(r.sample_rate, r.drain_capacity_frames),
            )
        });
        let result = result?;
        calls += 1;
        output.extend_from_slice(&drain[..result.frames * r.channels]);
        if result.complete {
            completed = true;
            break;
        }
    }
    let seconds = start.elapsed().as_secs_f64();
    if !completed || source_counts != plugin.qa_music_mask_counters() {
        return Err("native drain failed to finish/freeze musical model state".into());
    }
    if output.iter().any(|v| !v.is_finite() || v.abs() > 128.0) {
        return Err("native output software rail violation".into());
    }
    let frames = output.len() / r.channels;
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
        json!({"status":"ok","sample_rate":r.sample_rate,"channels":r.channels,"input_frames":r.frames,"output_frames":frames,
        "drain_frames":frames-r.frames,"drain_calls":calls,"drain_call_bound":bound,"native_refill_bound":refill_bound,"drain_capacity_frames":r.drain_capacity_frames,
        "callback_frames":r.callback_frames,"effective_parameters":r.parameters,"candidate_loaded":loaded,"load_error":load_error,
        "enabled":r.enabled,"counts":source_counts,"latency_samples":latency,"added_model_lookahead_samples":0,
        "sample_peak":peak,"qa_process_seconds":seconds,"deployment_admitted":false,
        "scope":"Native MCRA/classical history remains independent; QA gains are selected only at synthesis. Full finite drain freezes learned state. No normalization, fitted gain/delay, extra limiter or recorded-music acceptance."})
    );
    Ok(())
}
