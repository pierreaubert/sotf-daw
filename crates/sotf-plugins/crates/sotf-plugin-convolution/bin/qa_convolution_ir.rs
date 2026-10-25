//! Render an external IR through the public convolution processor for QA.

// Rust guideline compliant 2026-02-21

use serde::Deserialize;
use serde_json::json;
use sotf_host::{
    CountingAlloc, ParametricInPlacePluginAdapter, Plugin, ProcessContext, assert_no_allocs,
};
use sotf_plugin_convolution::{ConvolutionLoadStatus, ConvolutionPlugin, ConvolutionPluginParams};
use std::fs::{File, OpenOptions};
use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;

#[global_allocator]
static ALLOCATOR: CountingAlloc = CountingAlloc;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    ir: PathBuf,
    output: PathBuf,
    sample_rate: u32,
    channels: usize,
    ir_frames: usize,
    block_frames: usize,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let request_path = std::env::args_os()
        .nth(1)
        .ok_or("usage: qa-convolution-ir REQUEST.json")?;
    let mut bytes = Vec::new();
    File::open(request_path)?
        .take(65_537)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 65_536 {
        return Err("request exceeds QA size bound".into());
    }
    let request: Request = serde_json::from_slice(&bytes)?;
    if !(8_000..=96_000).contains(&request.sample_rate)
        || !(1..=32).contains(&request.channels)
        || request.ir_frames == 0
        || request.ir_frames > 1_440_000
        || request.ir_frames * request.channels > 8_000_000
        || !(1..=4_096).contains(&request.block_frames)
    {
        return Err("invalid bounded QA render geometry".into());
    }
    let inner = ConvolutionPlugin::from_params(
        request.channels,
        request.sample_rate,
        ConvolutionPluginParams {
            ir_file: request.ir.to_str().ok_or("IR path must be UTF-8")?.into(),
            mix: 1.0,
            gain_db: 0.0,
            use_nupc: false,
            ..Default::default()
        },
    )?;
    if inner.load_status() != ConvolutionLoadStatus::Ready {
        return Err("IR was not prepared".into());
    }
    let mut plugin = ParametricInPlacePluginAdapter::new(inner);
    plugin.initialize(f64::from(request.sample_rate))?;
    let latency = plugin.latency_samples();
    let calls = (request.ir_frames + latency + 2_048).div_ceil(request.block_frames);
    let mut input = vec![0.0; request.block_frames * request.channels];
    let mut output = vec![0.0; input.len()];
    let file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&request.output)?;
    let mut writer = BufWriter::new(file);
    for call in 0..calls {
        input.fill(0.0);
        if call == 0 {
            input[..request.channels].fill(1.0);
        }
        let context = ProcessContext::new(request.sample_rate, request.block_frames)
            .with_sample_position((call * request.block_frames) as u64);
        let mut processed = Ok(0);
        assert_no_allocs("external convolution IR QA", || {
            processed = plugin.process(&input, &mut output, &context);
        });
        if processed? != request.block_frames || output.iter().any(|x| !x.is_finite()) {
            return Err("invalid native IR render output".into());
        }
        for sample in &output {
            writer.write_all(&sample.to_le_bytes())?;
        }
    }
    writer.flush()?;
    println!(
        "{}",
        json!({"status":"ok", "latency_samples":latency,
        "channels":request.channels,"sample_rate":request.sample_rate,
        "frames":calls*request.block_frames,"callback_frames":request.block_frames,
        "scope":"UPC, diagonal/cyclic routing, full wet, unity gain, impulse on each channel"})
    );
    Ok(())
}
