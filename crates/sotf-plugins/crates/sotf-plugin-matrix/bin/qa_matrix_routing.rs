//! Render bounded routing proposals through the actual native Matrix plugin.

// Rust guideline compliant 2026-02-21
use serde::Deserialize;
use serde_json::json;
use sotf_host::{CountingAlloc, Plugin, ProcessContext, assert_no_allocs};
use sotf_plugin_matrix::MatrixPlugin;
use std::fs::File;
use std::io::Read;
use std::path::PathBuf;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

// Bounds cover sparse physical maps without accepting unbounded QA payloads.
const MAX_REQUEST_BYTES: usize = 8 * 1024 * 1024;
const MAX_FRAMES: usize = 2048;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Routing {
    input_channel_map: Vec<usize>,
    output_channel_map: Vec<usize>,
    matrix: Vec<f32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    sample_rate: u32,
    callback_frames: usize,
    params: Routing,
    input: Vec<f32>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-matrix-routing REQUEST.json")?;
    let mut bytes = Vec::new();
    File::open(PathBuf::from(path))?
        .take((MAX_REQUEST_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_REQUEST_BYTES {
        return Err("request exceeds QA size bound".into());
    }
    let request: Request = serde_json::from_slice(&bytes)?;
    let inputs = request.params.input_channel_map.len();
    let outputs = request.params.output_channel_map.len();
    if !(1..=16).contains(&inputs)
        || !(1..=16).contains(&outputs)
        || request.params.matrix.len() != inputs * outputs
        || !(44_100..=192_000).contains(&request.sample_rate)
        || !(1..=4096).contains(&request.callback_frames)
        || request.params.matrix.iter().any(|v| !v.is_finite())
        || request
            .params
            .matrix
            .chunks(inputs)
            .any(|row| row.iter().map(|v| f64::from(v.abs())).sum::<f64>() > 1.0 + 1e-6)
    {
        return Err("unsupported routing geometry, rate, or unit row-L1 bound".into());
    }
    let mut plugin = MatrixPlugin::with_sparse_mapping(
        request.params.input_channel_map.clone(),
        request.params.output_channel_map.clone(),
        request.params.matrix.clone(),
    )?;
    plugin.initialize(f64::from(request.sample_rate))?;
    let physical_inputs = plugin.input_channels();
    let physical_outputs = plugin.output_channels();
    if request.input.is_empty()
        || !request.input.len().is_multiple_of(physical_inputs)
        || request.input.len() / physical_inputs > MAX_FRAMES
        || request
            .input
            .iter()
            .any(|v| !v.is_finite() || v.abs() > 1.0)
    {
        return Err("input must contain bounded complete unit-amplitude frames".into());
    }
    let frames = request.input.len() / physical_inputs;
    let mut output = vec![f32::NAN; frames * physical_outputs];
    let mut position = 0;
    while position < frames {
        let count = (frames - position).min(request.callback_frames);
        let context =
            ProcessContext::new(request.sample_rate, count).with_sample_position(position as u64);
        let mut result = Ok(0);
        assert_no_allocs("Matrix routing process", || {
            result = plugin.process(
                &request.input[position * physical_inputs..(position + count) * physical_inputs],
                &mut output[position * physical_outputs..(position + count) * physical_outputs],
                &context,
            );
        });
        if result? != count {
            return Err("native process changed frame count".into());
        }
        position += count;
    }
    let mut result = None;
    assert_no_allocs("Matrix routing drain", || {
        result = Some(plugin.drain(&mut [], &ProcessContext::new(request.sample_rate, 0)));
    });
    let drain = result.expect("drain closure ran")?;
    if !drain.complete || drain.frames != 0 || plugin.latency_samples() != 0 {
        return Err("routing requires exact zero latency and no retained tail".into());
    }
    let peak = output
        .iter()
        .fold(0.0_f32, |peak, value| peak.max(value.abs()));
    if output.iter().any(|v| !v.is_finite()) || peak > 1.0 + 2e-6 {
        return Err("invalid native output or coherent unit bound exceeded".into());
    }
    println!(
        "{}",
        json!({"status":"ok", "sample_rate":request.sample_rate,
               "frames":frames, "physical_input_channels":physical_inputs,
               "physical_output_channels":physical_outputs,
               "latency_samples":0, "drained_frames":0, "sample_peak":peak,
               "output":output,
               "scope":"Actual sparse-mapped native Matrix; cold process and drain allocation guards; unit row-L1 QA policy"})
    );
    Ok(())
}
