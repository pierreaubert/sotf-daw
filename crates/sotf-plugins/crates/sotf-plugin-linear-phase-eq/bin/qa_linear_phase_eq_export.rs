//! Export qualified native FIR coefficients and their guarded impulse response.

// Rust guideline compliant 2026-02-21

use serde::Deserialize;
use serde_json::{Value, json};
use sotf_host::{
    CountingAlloc, ParametricInPlacePluginAdapter, Plugin, ProcessContext, assert_no_allocs,
};
use sotf_plugin_linear_phase_eq::{LinearPhaseEqPlugin, LinearPhaseEqPluginParams};
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    sample_rate: u32,
    params: LinearPhaseEqPluginParams,
    output_dir: PathBuf,
    frequencies_hz: Vec<f64>,
    callback_frames: usize,
}

fn write_f32(path: &Path, samples: &[f32]) -> Result<(), Box<dyn std::error::Error>> {
    let file = OpenOptions::new().write(true).create_new(true).open(path)?;
    let mut writer = BufWriter::new(file);
    for sample in samples {
        writer.write_all(&sample.to_le_bytes())?;
    }
    writer.flush()?;
    Ok(())
}

fn run() -> Result<Value, Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-linear-phase-eq-export REQUEST.json")?;
    let mut bytes = Vec::new();
    File::open(path)?.take(1_048_577).read_to_end(&mut bytes)?;
    if bytes.len() > 1_048_576 {
        return Err("request exceeds size limit".into());
    }
    let request: Request = serde_json::from_slice(&bytes)?;
    if !(44_100..=192_000).contains(&request.sample_rate)
        || !(1..=4_096).contains(&request.callback_frames)
        || request.frequencies_hz.is_empty()
        || request.frequencies_hz.len() > 4_096
        || request
            .frequencies_hz
            .iter()
            .any(|f| !f.is_finite() || *f < 0.0 || *f >= f64::from(request.sample_rate) / 2.0)
        || !request.params.accuracy_mode
        || request.params.phase_mode_index != 0
        || request.params.auto_gain
        || request.params.mix != 1.0
        || request
            .params
            .filters
            .iter()
            .any(|band| band.placement.is_some())
        || request.params.stereo_pairs.is_some()
    {
        return Err("export requires bounded mono linear/full-wet accuracy configuration".into());
    }
    let inner = LinearPhaseEqPlugin::from_params(1, request.sample_rate, request.params)?;
    let accuracy = inner
        .accuracy_report()
        .ok_or("missing automatic accuracy result")?;
    let coefficients = inner.stage_fir(0).ok_or("missing native FIR")?.to_vec();
    if inner.stage_count() != 1 || coefficients.len() != inner.selected_fir_taps() {
        return Err("export requires one shared FIR stage".into());
    }
    let response = request
        .frequencies_hz
        .iter()
        .map(|&frequency| {
            let value = inner
                .channel_complex_response(0, frequency)
                .ok_or("invalid native response frequency")?;
            Ok(json!([frequency, value.re, value.im]))
        })
        .collect::<Result<Vec<_>, &str>>()?;
    let mut plugin = ParametricInPlacePluginAdapter::new(inner);
    plugin.initialize(f64::from(request.sample_rate))?;
    let latency = plugin.latency_samples();
    let design_delay = coefficients.len() / 2;
    let partition_delay = latency.checked_sub(design_delay).ok_or("invalid latency")?;
    let frames = (coefficients.len() + partition_delay + 128).div_ceil(request.callback_frames)
        * request.callback_frames;
    let mut impulse = vec![0.0; frames];
    let mut input = vec![0.0; request.callback_frames];
    let mut output = vec![0.0; request.callback_frames];
    for (call, destination) in impulse
        .chunks_exact_mut(request.callback_frames)
        .enumerate()
    {
        input.fill(0.0);
        if call == 0 {
            input[0] = 1.0;
        }
        let context = ProcessContext::new(request.sample_rate, request.callback_frames)
            .with_sample_position((call * request.callback_frames) as u64);
        let mut processed = Ok(0);
        assert_no_allocs("offline learned FIR export native callback", || {
            processed = plugin.process(&input, &mut output, &context);
        });
        if processed? != request.callback_frames || output.iter().any(|x| !x.is_finite()) {
            return Err("invalid native impulse output".into());
        }
        destination.copy_from_slice(&output);
    }
    fs::create_dir(&request.output_dir)?;
    write_f32(&request.output_dir.join("fir.f32le"), &coefficients)?;
    write_f32(&request.output_dir.join("impulse.f32le"), &impulse)?;
    Ok(json!({"status":"ok","sample_rate":request.sample_rate,
        "selected_taps":coefficients.len(),"design_delay_samples":design_delay,
        "partition_delay_samples":partition_delay,"latency_samples":latency,
        "additional_latency_samples":accuracy.additional_latency_samples,
        "manual_latency_samples":accuracy.manual_latency_samples,
        "native_accuracy_maximum_error_db":accuracy.maximum_error_db,
        "native_accuracy_frequency_points":accuracy.frequency_points,
        "native_accuracy_worst_frequency_hz":accuracy.worst_frequency_hz,
        "callback_frames":request.callback_frames,"impulse_frames":frames,
        "response":response,"directory":request.output_dir}))
}

fn main() {
    match run() {
        Ok(result) => println!("{result}"),
        Err(error) => {
            println!("{}", json!({"status":"rejected","error":error.to_string()}));
            std::process::exit(2);
        }
    }
}
