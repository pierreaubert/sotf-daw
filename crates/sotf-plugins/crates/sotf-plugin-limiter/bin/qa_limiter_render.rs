//! Export bounded native Limiter renders with exact public drain alignment.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use serde_json::json;
use sotf_host::{CountingAlloc, ParametricInPlacePlugin, ProcessContext, assert_no_allocs};
use sotf_plugin_limiter::{LimiterPlugin, LimiterPluginParams};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

// Offline QA limits bound memory and accidental oversized training requests.
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
    params: serde_json::Value,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-limiter-render REQUEST.json")?;
    let mut bytes = Vec::new();
    File::open(path)?
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err("request exceeds size limit".into());
    }
    let request: Request = serde_json::from_slice(&bytes)?;
    let allowed = [
        "threshold_db",
        "release_ms",
        "lookahead_ms",
        "soft",
        "true_peak",
        "isp_mode",
        "dual_release",
        "mix",
        "feed_forward",
        "link_amount",
        "oversampling",
    ];
    if !request
        .params
        .as_object()
        .is_some_and(|object| object.keys().all(|key| allowed.contains(&key.as_str())))
    {
        return Err("params must be an object with known native fields".into());
    }
    let params: LimiterPluginParams = serde_json::from_value(request.params.clone())?;
    if !params.threshold_db.is_finite()
        || !(-20.0..=0.0).contains(&params.threshold_db)
        || !params.release_ms.is_finite()
        || !(10.0..=1000.0).contains(&params.release_ms)
        || !params.lookahead_ms.is_finite()
        || !(0.0..=20.0).contains(&params.lookahead_ms)
        || !params.link_amount.is_finite()
        || !(0.0..=1.0).contains(&params.link_amount)
        || params.oversampling > 2
        || params.soft
        || params.mix != 1.0
    {
        return Err(
            "QA requires bounded native parameters, hard limiting, and fully wet output".into(),
        );
    }

    let samples = request
        .frames
        .checked_mul(request.channels)
        .ok_or("sample count overflow")?;
    if !(8_000..=192_000).contains(&request.sample_rate)
        || !(1..=8).contains(&request.channels)
        || samples == 0
        || samples > MAX_SAMPLES
        || !(1..=4_096).contains(&request.callback_frames)
    {
        return Err("unsupported render geometry".into());
    }
    let expected_bytes = samples * 4;
    bytes.clear();
    File::open(&request.input)?
        .take((expected_bytes + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() != expected_bytes {
        return Err("input must contain exactly frames*channels float32-le samples".into());
    }
    let input: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .copied()
        .map(f32::from_le_bytes)
        .collect();
    if input
        .iter()
        .any(|sample| !sample.is_finite() || sample.abs() > 32.0)
    {
        return Err("input samples must be finite and within +/-32".into());
    }
    let mut plugin = LimiterPlugin::from_params(request.channels, params.clone());
    plugin.initialize(f64::from(request.sample_rate))?;
    let latency = plugin.latency_samples();
    let mut output = Vec::with_capacity((request.frames + latency) * request.channels);
    let mut block = vec![0.0; request.callback_frames * request.channels];
    for (index, source) in input.chunks(block.len()).enumerate() {
        block[..source.len()].copy_from_slice(source);
        let frames = source.len() / request.channels;
        let context = ProcessContext::new(request.sample_rate, frames)
            .with_sample_position((index * request.callback_frames) as u64);
        let mut result = Ok(0);
        assert_no_allocs("Limiter export process", || {
            result = plugin.process_in_place(&mut block[..source.len()], &context);
        });
        if result? != frames {
            return Err("native process changed frame count".into());
        }
        output.extend_from_slice(&block[..source.len()]);
    }
    let capacity = plugin.drain_output_frames_max();
    let mut tail = vec![0.0; capacity * request.channels];
    let drain_context = ProcessContext::new(request.sample_rate, 0);
    let mut begun = Ok(());
    assert_no_allocs("Limiter export begin drain", || {
        begun = plugin.begin_drain(&drain_context);
    });
    begun?;
    let call_bound = plugin
        .drain_call_bound()
        .ok_or("missing drain bound")?
        .get();
    if call_bound > 65_536 {
        return Err("native drain bound exceeds QA limit".into());
    }
    let mut complete = false;
    let mut drained = 0;
    // Oversampled drain can advance filter phases without emitting frames.
    // Honor its public bound rather than assuming every call produces audio.
    for _ in 0..call_bound {
        let context = ProcessContext::new(request.sample_rate, capacity)
            .with_sample_position((request.frames + drained) as u64);
        let mut result = None;
        assert_no_allocs("Limiter export drain", || {
            result = Some(plugin.drain(&mut tail, &context));
        });
        let result = result.expect("drain closure ran")?;
        if result.frames > capacity {
            return Err("native drain exceeded capacity".into());
        }
        drained += result.frames;
        if drained > 65_536 {
            return Err("native drain exceeded QA output limit".into());
        }
        output.extend_from_slice(&tail[..result.frames * request.channels]);
        if result.complete {
            complete = true;
            break;
        }
    }
    if !complete
        || drained < latency
        || output.len() != (request.frames + drained) * request.channels
        || output.iter().any(|sample| !sample.is_finite())
    {
        return Err("invalid native output or drain alignment".into());
    }
    let peak = output
        .iter()
        .fold(0.0_f32, |peak, sample| peak.max(sample.abs()));
    let ceiling = 10.0_f32.powf(params.threshold_db / 20.0);
    // Numerical slack covers the native float32 gain and independent powf conversion.
    if peak > ceiling + 2.0e-6 {
        return Err("native output exceeded deterministic sample ceiling".into());
    }
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&request.output)?;
    let mut writer = BufWriter::new(file);
    for sample in output {
        writer.write_all(&sample.to_le_bytes())?;
    }
    writer.flush()?;
    println!(
        "{}",
        json!({"status":"ok", "sample_rate":request.sample_rate,
               "channels":request.channels,"input_frames":request.frames,
               "output_frames":request.frames + drained,"latency_samples":latency,
               "drain_call_bound":call_bound,"tail_after_aligned_input":drained-latency,
               "drained_frames":drained,"callback_frames":request.callback_frames,
               "requested_params":params,"sample_peak":peak,"sample_ceiling":ceiling,
               "scope":"native public Limiter, untrimmed latency prefix and exact drain; hard fully-wet sample ceiling checked; no learned processing or independent true-peak certification"})
    );
    Ok(())
}
