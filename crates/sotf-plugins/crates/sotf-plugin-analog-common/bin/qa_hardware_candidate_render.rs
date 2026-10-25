//! Render unadmitted candidate qualification with native continuously running fallback.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use serde_json::{Value, json};
use sotf_host::CountingAlloc;
use sotf_host::test_utils::assert_no_allocs_or_deallocs;
use sotf_plugin_analog_common::AnalogColorStage;
use sotf_plugin_analog_common::hardware_candidate::{
    CandidateCounters, HardwareCandidate, HardwareQualification, MAX_CANDIDATE_BYTES,
};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;
use std::time::Instant;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;
const MAX_SAMPLES: usize = 4_000_000;
const MAX_REQUEST_BYTES: usize = 65_536;

#[derive(Deserialize, serde::Serialize)]
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
struct CandidateFile {
    path: PathBuf,
    sha256: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    input: PathBuf,
    output: PathBuf,
    candidate: Option<CandidateFile>,
    expected_context: Value,
    sample_rate: u32,
    frames: usize,
    callback_frames: usize,
    appended_zero_frames: usize,
    enabled: bool,
    disabled_ranges: Vec<[usize; 2]>,
    reset_frames: Vec<usize>,
    fallback: Controls,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-hardware-candidate-render REQUEST.json")?;
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
    if !(44100..=192000).contains(&r.sample_rate)
        || r.frames == 0
        || frames > MAX_SAMPLES
        || !(1..=4096).contains(&r.callback_frames)
        || r.appended_zero_frames > 65536
        || r.disabled_ranges.len() > 64
        || r.reset_frames.len() > 64
        || r.disabled_ranges
            .iter()
            .any(|v| v[0] >= v[1] || v[1] > frames)
        || r.disabled_ranges.windows(2).any(|v| v[0][1] > v[1][0])
        || r.reset_frames.iter().any(|&p| p == 0 || p >= frames)
        || r.reset_frames.windows(2).any(|v| v[0] >= v[1])
    {
        return Err("unsupported bounded qualification geometry or event timeline".into());
    }
    let expected_context = serde_json::to_vec(&r.expected_context)?;
    let mut load_error = None;
    let candidate = if let Some(spec) = &r.candidate {
        let result = (|| -> Result<HardwareCandidate, String> {
            let mut bytes = Vec::new();
            File::open(&spec.path)
                .map_err(|e| e.to_string())?
                .take((MAX_CANDIDATE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)
                .map_err(|e| e.to_string())?;
            let loaded = HardwareCandidate::from_json(&bytes, &spec.sha256, &expected_context)
                .map_err(|e| e.to_string())?;
            if loaded.sample_rate() != r.sample_rate {
                return Err("candidate capture rate differs from render rate".into());
            }
            Ok(loaded)
        })();
        match result {
            Ok(candidate) => Some(candidate),
            Err(e) => {
                load_error = Some(e);
                None
            }
        }
    } else {
        None
    };
    let loaded = candidate.is_some();
    let p = &r.fallback;
    if p.model_id > 5
        || !p.drive_db.is_finite()
        || !(-60.0..=36.0).contains(&p.drive_db)
        || !p.color.is_finite()
        || !(0.0..=1.0).contains(&p.color)
        || !p.character.is_finite()
        || !(0.0..=1.0).contains(&p.character)
        || !p.output_trim_db.is_finite()
        || !(-24.0..=24.0).contains(&p.output_trim_db)
    {
        return Err("invalid complete classical fallback controls".into());
    }
    let mut fallback = AnalogColorStage::new(1);
    fallback.set_model_id(p.model_id)?;
    fallback.set_drive_db(p.drive_db)?;
    fallback.set_color(p.color)?;
    fallback.set_character(p.character)?;
    fallback.set_output_trim_db(p.output_trim_db)?;
    let mut processor = HardwareQualification::new(fallback, candidate, r.sample_rate)?;
    assert_no_allocs_or_deallocs("qualification fresh reset", || processor.reset());
    let mut bytes = Vec::new();
    File::open(&r.input)?
        .take((r.frames * 4 + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() != r.frames * 4 {
        return Err("PCM byte count mismatch".into());
    }
    // Deliberately retain all float32 patterns to qualify native nonfinite recovery.
    let input: Vec<_> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .copied()
        .map(f32::from_le_bytes)
        .collect();
    let mut events: Vec<_> = r
        .disabled_ranges
        .iter()
        .flatten()
        .copied()
        .chain(r.reset_frames.iter().copied())
        .chain([frames])
        .collect();
    events.sort_unstable();
    events.dedup();
    let mut epoch_counts = Vec::with_capacity(r.reset_frames.len() + 1);
    let mut block = vec![0.0; r.callback_frames];
    let mut output = Vec::with_capacity(frames);
    let mut position = 0;
    let start = Instant::now();
    while position < frames {
        if r.reset_frames.binary_search(&position).is_ok() {
            epoch_counts.push(processor.counters());
            assert_no_allocs_or_deallocs("qualification epoch reset", || processor.reset());
        }
        let enabled = r.enabled
            && !r
                .disabled_ranges
                .iter()
                .any(|v| v[0] <= position && position < v[1]);
        assert_no_allocs_or_deallocs("qualification enable", || processor.set_enabled(enabled));
        let next = events
            .iter()
            .copied()
            .find(|&v| v > position)
            .unwrap_or(frames);
        let count = (frames - position)
            .min(r.callback_frames)
            .min(next - position);
        block[..count].fill(0.0);
        let real = r.frames.saturating_sub(position).min(count);
        if real > 0 {
            block[..real].copy_from_slice(&input[position..position + real]);
        }
        let mut result = Ok(());
        assert_no_allocs_or_deallocs("qualification native process", || {
            result = processor.process(&mut block[..count], r.sample_rate)
        });
        result?;
        output.extend_from_slice(&block[..count]);
        position += count;
    }
    let seconds = start.elapsed().as_secs_f64();
    epoch_counts.push(processor.counters());
    let totals = epoch_counts
        .iter()
        .fold(CandidateCounters::default(), |mut total, c| {
            total.neural += c.neural;
            total.disabled += c.disabled;
            total.unavailable += c.unavailable;
            total.out_of_domain += c.out_of_domain;
            total.numerical += c.numerical;
            total
        });
    if output.iter().any(|v| !v.is_finite() || v.abs() > 128.0) {
        return Err("native safety rail violation".into());
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
        json!({"status":"ok","sample_rate":r.sample_rate,"channels":1,"input_frames":r.frames,
        "output_frames":frames,"callback_frames":r.callback_frames,"appended_zero_frames":r.appended_zero_frames,
        "candidate_loaded":loaded,"load_error":load_error,"counts":totals,"epoch_counts":epoch_counts,"effective_fallback":r.fallback,
        "latency_samples":0,"sample_peak":peak,"qa_process_seconds":seconds,"hardware_model_admitted":false,
        "scope":"QA-only unadmitted candidate with continuously running native classical fallback. Immediate switching; no click-free live activation or production admission. No normalization, limiting or finite-zero-tail claim. Timing includes QA guards, fallback, candidate and copies."})
    );
    Ok(())
}
