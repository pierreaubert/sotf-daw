//! Render the native coloration baseline for explicitly calibrated hardware fitting.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use serde_json::json;
use sotf_host::CountingAlloc;
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_plugin_analog_common::{AnalogColorStage, MODEL_NAMES, REFERENCE_LEVEL_DBFS};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;
use std::time::Instant;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;
// Bound programme plus zero-extension storage; zeros do not establish finite support.
const MAX_SAMPLES: usize = 4_000_000;
const MAX_REQUEST_BYTES: usize = 65_536;
const MAX_ZERO_FRAMES: usize = 65_536;

#[derive(Clone, Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct Controls {
    model_id: u32,
    drive_db: f32,
    color: f32,
    character: f32,
    output_trim_db: f32,
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
    prepared_maximum_frames: usize,
    appended_zero_frames: usize,
    controls: Controls,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-analog-color-render REQUEST.json")?;
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
        || !(1..=4096).contains(&r.prepared_maximum_frames)
        || r.appended_zero_frames > MAX_ZERO_FRAMES
    {
        return Err("unsupported bounded native coloration geometry".into());
    }
    let p = &r.controls;
    if p.model_id as usize >= MODEL_NAMES.len()
        || !p.drive_db.is_finite()
        || !p.color.is_finite()
        || !p.character.is_finite()
        || !p.output_trim_db.is_finite()
        || !(-60.0..=36.0).contains(&p.drive_db)
        || !(0.0..=1.0).contains(&p.color)
        || !(0.0..=1.0).contains(&p.character)
        || !(-24.0..=24.0).contains(&p.output_trim_db)
    {
        return Err("controls must satisfy the shared production parameter schema".into());
    }
    let mut stage = AnalogColorStage::new(r.channels);
    stage.set_model_id(p.model_id)?;
    stage.set_drive_db(p.drive_db)?;
    stage.set_color(p.color)?;
    stage.set_character(p.character)?;
    stage.set_output_trim_db(p.output_trim_db)?;
    stage.prepare(r.sample_rate, r.prepared_maximum_frames)?;
    assert_no_allocs_or_deallocs("native coloration fresh reset", || stage.reset());
    let mut bytes = Vec::new();
    let real_samples = r.frames * r.channels;
    File::open(&r.input)?
        .take((real_samples * 4 + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() != real_samples * 4 {
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
        return Err("comparison PCM must be finite within native input rails+/-16".into());
    }
    let mut block = vec![0.0; r.callback_frames * r.channels];
    let mut output = Vec::with_capacity(samples);
    let start = Instant::now();
    for position in (0..frames).step_by(r.callback_frames) {
        let count = (frames - position).min(r.callback_frames);
        let used = count * r.channels;
        block[..used].fill(0.0);
        let real = r.frames.saturating_sub(position).min(count) * r.channels;
        if real > 0 {
            block[..real]
                .copy_from_slice(&input[position * r.channels..position * r.channels + real]);
        }
        let mut result = Ok(());
        assert_no_allocs_or_deallocs("native coloration process", || {
            result = stage.process_interleaved(&mut block[..used], count);
        });
        result?;
        output.extend_from_slice(&block[..used]);
    }
    let seconds = start.elapsed().as_secs_f64();
    if output.iter().any(|v| !v.is_finite()) {
        return Err("non-finite native output".into());
    }
    let peak = output.iter().fold(0.0_f32, |a, b| a.max(b.abs()));
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
        json!({"status":"ok","sample_rate":r.sample_rate,"channels":r.channels,
        "input_frames":r.frames,"output_frames":frames,"appended_zero_frames":r.appended_zero_frames,
        "callback_frames":r.callback_frames,"prepared_maximum_frames":r.prepared_maximum_frames,
        "latency_samples":stage.latency_samples(),"effective_controls":r.controls,
        "model_name":stage.model_name(),"nominal_reference_level_dbfs":REFERENCE_LEVEL_DBFS,
        "sample_peak":peak,"qa_process_seconds":seconds,"tail":"not_claimed_finite",
        "scope":"Native shared coloration, fresh static controls and no defects added. Nominal reference label is not a hardware calibration. Timing includes QA guards and output copies; not a production performance guarantee. No learned model, normalization, gain alignment, limiting or finite-tail claim."})
    );
    Ok(())
}
