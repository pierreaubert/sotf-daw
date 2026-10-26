//! Export actual causal onset features and native shaped output without lookahead.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use serde_json::json;
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, ParametricInPlacePlugin, ProcessContext};
use sotf_plugin_transient_shaper::onset::{OnsetModel, OnsetSnapshot};
use sotf_plugin_transient_shaper::{TransientShaperPlugin, TransientShaperPluginParams};
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
    features_output: PathBuf,
    frames: usize,
    channels: usize,
    sample_rate: u32,
    callback_frames: usize,
    parameters: TransientShaperPluginParams,
    model: Option<ModelFile>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-onset-assist-render REQUEST.json")?;
    let mut bytes = Vec::new();
    File::open(path)?.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err("request exceeds64KiB".into());
    }
    let r: Request = serde_json::from_slice(&bytes)?;
    if !(44100..=192000).contains(&r.sample_rate)
        || !(1..=2).contains(&r.channels)
        || !(1..=1_000_000).contains(&r.frames)
        || !(1..=64).contains(&r.callback_frames)
    {
        return Err("unsupported bounded onset geometry".into());
    }
    let mut model_error = None;
    let model = if let Some(file) = &r.model {
        let result = (|| -> Result<OnsetModel, String> {
            let mut raw = Vec::new();
            File::open(&file.path)
                .map_err(|e| e.to_string())?
                .take(65537)
                .read_to_end(&mut raw)
                .map_err(|e| e.to_string())?;
            OnsetModel::from_json(&raw, &file.sha256)
        })();
        match result {
            Ok(model) => Some(model),
            Err(e) => {
                model_error = Some(e);
                None
            }
        }
    } else {
        None
    };
    let model_loaded = model.is_some();
    let mut plugin = TransientShaperPlugin::from_params(r.channels, r.parameters.clone())?;
    plugin.initialize(f64::from(r.sample_rate))?;
    plugin.configure_onset_assist(model)?;
    assert_no_allocs_or_deallocs("onset renderer reset", || plugin.reset());
    let hop = plugin
        .onset_assist_snapshot()
        .ok_or("missing onset state")?
        .hop_frames;
    let samples = r.frames * r.channels;
    bytes.clear();
    File::open(&r.input)?
        .take((samples * 4 + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() != samples * 4 {
        return Err("input size mismatch".into());
    }
    let mut output: Vec<_> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| f32::from_le_bytes(*v))
        .collect();
    if output.iter().any(|v| !v.is_finite() || v.abs() > 64.0) {
        return Err("input exceeds bounded native domain".into());
    }
    let mut rows: Vec<(usize, usize, OnsetSnapshot)> = Vec::with_capacity(r.frames / hop);
    let mut copied = 0;
    let mut accepted = 0;
    for chunk in output.chunks_mut(r.callback_frames * r.channels) {
        let frames = chunk.len() / r.channels;
        let mut result = Ok(0);
        assert_no_allocs_or_deallocs("onset renderer process", || {
            result = plugin.process_in_place(chunk, &ProcessContext::new(r.sample_rate, frames))
        });
        if result? != frames {
            return Err("native frame count mismatch".into());
        }
        accepted += frames;
        let snapshot = plugin.onset_assist_snapshot().ok_or("missing snapshot")?;
        let hops = snapshot.counters.hops as usize;
        if hops != copied {
            if hops != copied + 1 {
                return Err("skipped native analysis hop".into());
            }
            rows.push((hops * hop, accepted, snapshot));
            copied = hops;
        }
    }
    let final_state = plugin
        .onset_assist_snapshot()
        .ok_or("missing final state")?;
    if copied != r.frames / hop
        || final_state.counters.frames != r.frames as u64
        || output.iter().any(|v| !v.is_finite())
    {
        return Err("native source/feature domain mismatch".into());
    }
    let mut writer = BufWriter::new(
        OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&r.output)?,
    );
    for value in &output {
        writer.write_all(&value.to_le_bytes())?;
    }
    writer.flush()?;
    let metadata = json!({"sample_rate":r.sample_rate,"channels":r.channels,"source_frames":r.frames,"output_frames":r.frames,
        "callback_frames":r.callback_frames,"hop_frames":hop,"latency_samples":plugin.latency_samples(),"added_model_lookahead_samples":0,
        "parameters":r.parameters,"model_loaded":model_loaded,"model_error":model_error,"model_sha256":r.model.as_ref().map(|m|&m.sha256),
        "rows":rows.iter().map(|(end,observed,state)|json!({"end_frame":end,"observed_after_frame":observed,"state":state})).collect::<Vec<_>>(),
        "final_state":final_state,"deployment_admitted":false});
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&r.features_output)?;
    serde_json::to_writer(&mut file, &metadata)?;
    file.write_all(b"\n")?;
    println!(
        "{}",
        json!({"feature_hops":rows.len(),"output_frames":r.frames,"latency_samples":plugin.latency_samples(),"deployment_admitted":false})
    );
    Ok(())
}
