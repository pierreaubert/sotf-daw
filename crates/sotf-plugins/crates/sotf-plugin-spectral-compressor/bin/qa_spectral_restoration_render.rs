//! Render native spectral restoration output and actual conditional gain traces.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use serde_json::json;
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_spectral_compressor::restoration::DenoisingMask;
use sotf_plugin_spectral_compressor::{SpectralCompressorPlugin, SpectralCompressorPluginParams};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

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
    gain_trace_output: PathBuf,
    sample_rate: u32,
    channels: usize,
    frames: usize,
    callback_frames: usize,
    drain_capacity_frames: usize,
    parameters: SpectralCompressorPluginParams,
    enabled: bool,
    model: Option<ModelFile>,
}

fn write_f32(path: &Path, samples: &[f32]) -> std::io::Result<()> {
    let mut writer = BufWriter::new(OpenOptions::new().write(true).create_new(true).open(path)?);
    for sample in samples {
        writer.write_all(&sample.to_le_bytes())?;
    }
    writer.flush()
}

fn capture(
    plugin: &SpectralCompressorPlugin,
    trace: &mut [f32],
    copied: &mut usize,
) -> Result<(), String> {
    let (hops, gains, _) = plugin.restoration_synthesis_snapshot();
    if hops == *copied {
        return Ok(());
    }
    if hops != *copied + 1 || (hops * gains.len()) > trace.len() {
        return Err("trace skipped a native hop or exceeded storage".into());
    }
    if gains
        .iter()
        .any(|v| !v.is_finite() || !(0.0..=1.000001).contains(v))
    {
        return Err("invalid native synthesis gain".into());
    }
    trace[*copied * gains.len()..hops * gains.len()].copy_from_slice(gains);
    *copied = hops;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-spectral-restoration-render REQUEST.json")?;
    let mut raw = Vec::new();
    File::open(path)?.take(65537).read_to_end(&mut raw)?;
    if raw.len() > 65536 {
        return Err("request exceeds64KiB".into());
    }
    let r: Request = serde_json::from_slice(&raw)?;
    if !(44100..=192000).contains(&r.sample_rate)
        || !(1..=2).contains(&r.channels)
        || !(1..=1_000_000).contains(&r.frames)
        || !(1..=256).contains(&r.callback_frames)
        || !(1..=256).contains(&r.drain_capacity_frames)
        || r.parameters.fft_size_index != 0
        || r.parameters.delta_listen
        || r.parameters.mix != 1.0
    {
        return Err("use bounded mono/stereo full-wet spectral rendering".into());
    }
    let mut load_error = None;
    let model = if let Some(file) = &r.model {
        let loaded = (|| -> Result<DenoisingMask, String> {
            let mut bytes = Vec::new();
            File::open(&file.path)
                .map_err(|e| e.to_string())?
                .take(65537)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            DenoisingMask::from_json(&bytes, &file.sha256)
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
    let model_loaded = model.is_some();
    let mut plugin = SpectralCompressorPlugin::try_from_params(r.channels, r.parameters.clone())?;
    plugin.initialize(f64::from(r.sample_rate))?;
    if r.enabled {
        plugin.configure_restoration(model)?;
    }
    assert_no_allocs_or_deallocs("spectral restoration renderer reset", || plugin.reset());
    if plugin.latency_samples() != 1024 {
        return Err("native spectral latency changed".into());
    }
    let source_samples = r.frames * r.channels;
    raw.clear();
    File::open(&r.input)?
        .take((source_samples * 4 + 1) as u64)
        .read_to_end(&mut raw)?;
    if raw.len() != source_samples * 4 {
        return Err("input size mismatch".into());
    }
    let mut output: Vec<f32> = raw
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| f32::from_le_bytes(*v))
        .collect();
    if output.iter().any(|v| !v.is_finite() || v.abs() > 16.0) {
        return Err("unsupported source amplitude".into());
    }
    let expected_tail = 2048 - 256 + (256 - r.frames % 256) % 256;
    output.reserve(expected_tail * r.channels);
    let maximum_hops = (r.frames + expected_tail) / 256 + 1;
    let mut trace = vec![0.0; maximum_hops * r.channels * 513];
    let mut copied = 0;
    for block in output[..source_samples].chunks_mut(r.callback_frames * r.channels) {
        let context = ProcessContext::new(r.sample_rate, block.len() / r.channels);
        let mut result = Ok(0);
        assert_no_allocs_or_deallocs("spectral restoration renderer process", || {
            result = plugin.process_in_place(block, &context)
        });
        result?;
        capture(&plugin, &mut trace, &mut copied)?;
    }
    let source_counters = plugin.restoration_counters();
    if source_counters.is_some_and(|c| c.source_hops != (r.frames / 256) as u64) {
        return Err("source hop counter mismatch".into());
    }
    let mut tail = vec![0.0; r.drain_capacity_frames * r.channels];
    let mut tail_frames = 0;
    let mut complete = false;
    for _ in 0..=expected_tail {
        let mut result = Ok(sotf_host::plugin::PluginDrainResult::COMPLETE);
        assert_no_allocs_or_deallocs("spectral restoration renderer drain", || {
            result = plugin.drain(&mut tail, &ProcessContext::new(r.sample_rate, 0))
        });
        let done = result?;
        tail_frames += done.frames;
        output.extend_from_slice(&tail[..done.frames * r.channels]);
        capture(&plugin, &mut trace, &mut copied)?;
        if done.complete {
            complete = true;
            break;
        }
    }
    if !complete
        || tail_frames != expected_tail
        || plugin.restoration_counters() != source_counters
        || output.iter().any(|v| !v.is_finite() || v.abs() > 64.0)
    {
        return Err("finite drain, frozen counter or output domain failure".into());
    }
    write_f32(&r.output, &output)?;
    write_f32(&r.gain_trace_output, &trace[..copied * r.channels * 513])?;
    let (_, _, window) = plugin.restoration_synthesis_snapshot();
    println!(
        "{}",
        json!({"source_frames":r.frames,"output_frames":output.len()/r.channels,
        "sample_rate":r.sample_rate,"channels":r.channels,"callback_frames":r.callback_frames,
        "drain_capacity_frames":r.drain_capacity_frames,"drain_frames":tail_frames,
        "fft_size":1024,"hop_size":256,"latency_samples":1024,"added_model_lookahead_samples":0,
        "trace_hops":copied,"trace_layout":"hop/channel/bin","analysis_window":window,
        "parameters":r.parameters,"enabled":r.enabled,"model_loaded":model_loaded,"model_error":load_error,
        "model_sha256":r.model.as_ref().map(|m|&m.sha256),"source_counters":source_counters,
        "deployment_admitted":false})
    );
    Ok(())
}
