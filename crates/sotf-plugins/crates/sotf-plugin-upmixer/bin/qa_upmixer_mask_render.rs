//! Export native source decisions and complete multichannel output with bounded I/O.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use serde_json::json;
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_host::{CountingAlloc, Plugin, ProcessContext};
use sotf_plugin_upmixer::source_mask::{SourceMaskModel, SourceMaskSnapshot};
use sotf_plugin_upmixer::{UpmixerPlugin, UpmixerPluginParams};
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
    callback_frames: usize,
    speaker_config: String,
    model: Option<ModelFile>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-upmixer-mask-render REQUEST.json")?;
    let mut bytes = Vec::new();
    File::open(path)?.take(65537).read_to_end(&mut bytes)?;
    if bytes.len() > 65536 {
        return Err("request exceeds64KiB".into());
    }
    let r: Request = serde_json::from_slice(&bytes)?;
    if !(1..=1_000_000).contains(&r.frames)
        || !(1..=256).contains(&r.callback_frames)
        || !["5.1", "7.1"].contains(&r.speaker_config.as_str())
    {
        return Err("unsupported bounded Upmixer render geometry".into());
    }
    let mut model_error = None;
    let model = if let Some(file) = &r.model {
        let loaded = (|| -> Result<SourceMaskModel, String> {
            let mut raw = Vec::new();
            File::open(&file.path)
                .map_err(|e| e.to_string())?
                .take(65537)
                .read_to_end(&mut raw)
                .map_err(|e| e.to_string())?;
            SourceMaskModel::from_json(&raw, &file.sha256)
        })();
        match loaded {
            Ok(m) => Some(m),
            Err(e) => {
                model_error = Some(e);
                None
            }
        }
    } else {
        None
    };
    let model_loaded = model.is_some();
    let mut params = UpmixerPluginParams::default();
    params.core.fft_size = 512;
    params.core.enable_hr_direct = false;
    params.core.speaker_config = r.speaker_config.clone();
    // Freeze explicit analysis settings. Native final safety remains enabled.
    params.output.auto_gain_enabled = false;
    params.dialogue.dialogue_weight = 0.0;
    let mut plugin = UpmixerPlugin::from_params(params.clone());
    plugin.initialize(48000.0)?;
    plugin.configure_source_masks(model)?;
    assert_no_allocs_or_deallocs("Upmixer mask renderer reset", || plugin.reset());
    bytes.clear();
    File::open(&r.input)?
        .take((r.frames * 8 + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() != r.frames * 8 {
        return Err("input size mismatch".into());
    }
    let input: Vec<_> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|v| f32::from_le_bytes(*v))
        .collect();
    if input.iter().any(|v| !v.is_finite() || v.abs() > 64.0) {
        return Err("input exceeds bounded domain".into());
    }
    let channels = plugin.output_channels();
    let mut block = vec![0.0; r.callback_frames * channels];
    let mut output = Vec::with_capacity((r.frames + 65536) * channels);
    let mut rows: Vec<(usize, usize, SourceMaskSnapshot)> = Vec::with_capacity(r.frames / 256);
    let mut accepted = 0;
    let mut copied = 0;
    for chunk in input.chunks(r.callback_frames * 2) {
        let frames = chunk.len() / 2;
        let mut processed = Ok(0);
        assert_no_allocs_or_deallocs("Upmixer mask renderer process", || {
            processed = plugin.process(
                chunk,
                &mut block[..frames * channels],
                &ProcessContext::new(48000, frames),
            );
        });
        if processed? != frames {
            return Err("native source frame count mismatch".into());
        }
        output.extend_from_slice(&block[..frames * channels]);
        accepted += frames;
        let snapshot = plugin.source_mask_snapshot();
        let count = snapshot.analysis_frames as usize;
        if count != copied {
            if count != copied + 1 {
                return Err("skipped native analysis frame".into());
            }
            rows.push((count * 256, accepted, snapshot));
            copied = count;
        }
    }
    if copied != r.frames / 256 {
        return Err("native analysis geometry mismatch".into());
    }
    let final_state = plugin.source_mask_snapshot();
    let mut complete = false;
    // Bound all post-source generation; copying outside the guarded callback is offline I/O.
    for _ in 0..65536 {
        let mut drained = None;
        assert_no_allocs_or_deallocs("Upmixer mask renderer drain", || {
            drained =
                Some(plugin.drain(&mut block, &ProcessContext::new(48000, r.callback_frames)));
        });
        let result = drained.ok_or("missing drain result")??;
        output.extend_from_slice(&block[..result.frames * channels]);
        if output.len() / channels > r.frames + 65536 {
            return Err("native tail exceeded export bound".into());
        }
        if result.complete {
            complete = true;
            break;
        }
    }
    if !complete
        || output.iter().any(|v| !v.is_finite())
        || plugin.source_mask_snapshot().analysis_frames != final_state.analysis_frames
    {
        return Err("invalid native tail or changed source analysis".into());
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
    let metadata = json!({"sample_rate":48000,"channels":channels,"source_frames":r.frames,
        "speaker_positions":sotf_host::speaker_config::get_speaker_config(&r.speaker_config).ok_or("missing speaker layout")?.speakers,
        "output_frames":output.len()/channels,"callback_frames":r.callback_frames,
        "hop_frames":256,"latency_samples":plugin.latency_samples(),"model_lookahead_samples":0,
        "parameters":params,"model_loaded":model_loaded,"model_error":model_error,
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
        json!({"feature_frames":rows.len(),"output_frames":output.len()/channels,"channels":channels,"latency_samples":plugin.latency_samples()})
    );
    Ok(())
}
