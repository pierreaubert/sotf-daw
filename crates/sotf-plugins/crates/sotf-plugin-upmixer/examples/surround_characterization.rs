//! Render supplied interleaved float samples for offline surround characterization.
//!
//! Inputs and outputs are little-endian f32 files. No audio device is opened.

use sotf_host::speaker_config::{ChannelLayout, get_speaker_config};
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_upmixer::{UpmixerPlugin, UpmixerPluginParams};
use std::io::{BufWriter, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 7 {
        return Err(
            "expected INPUT_LAYOUT OUTPUT_LAYOUT RATE BYPASS_DECORRELATION INPUT_F32 OUTPUT_F32"
                .into(),
        );
    }
    let sample_rate: u32 = args[3].parse()?;
    let bypass_decorrelation: bool = args[4].parse()?;
    let layout = ChannelLayout::from_speaker_config(
        get_speaker_config(&args[1]).ok_or("unknown input layout")?,
    )?;
    let channels = layout.channels.len();
    let bytes = std::fs::read(&args[5])?;
    if bytes.len() % (channels * std::mem::size_of::<f32>()) != 0 {
        return Err("input file does not contain whole interleaved frames".into());
    }
    let input: Vec<f32> = bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|value| f32::from_le_bytes(*value))
        .collect();
    if input.iter().any(|value| !value.is_finite()) {
        return Err("input contains non-finite samples".into());
    }
    let mut params = UpmixerPluginParams::default();
    params.core.speaker_config.clone_from(&args[2]);
    params.multichannel.input_layout = Some(layout);
    params.bypass.bypass_decorrelation = bypass_decorrelation;
    let fft_size = params.core.fft_size;
    let mut plugin = UpmixerPlugin::try_from_params(params)?;
    plugin.initialize(f64::from(sample_rate))?;
    let output_channels = plugin.output_channels();
    let mut output = vec![0.0_f32; 256 * output_channels];
    let mut writer = BufWriter::new(std::fs::File::create(&args[6])?);
    let frames = input.len() / channels;
    for start in (0..frames).step_by(256) {
        let count = (frames - start).min(256);
        let written = plugin.process(
            &input[start * channels..(start + count) * channels],
            &mut output[..count * output_channels],
            &ProcessContext::new(sample_rate, count),
        )?;
        if written != count
            || output[..count * output_channels]
                .iter()
                .any(|value| !value.is_finite())
        {
            return Err("render failed or produced non-finite samples".into());
        }
        for value in &output[..count * output_channels] {
            writer.write_all(&value.to_le_bytes())?;
        }
    }
    writer.flush()?;
    println!(
        "{}",
        serde_json::json!({
            "input_layout": args[1], "output_layout": args[2], "sample_rate": sample_rate,
            "bypass_decorrelation": bypass_decorrelation, "input_channels": channels,
            "output_channels": output_channels, "frames": frames,
            "latency_frames": plugin.latency_samples(), "fft_size": fft_size,
            "block_frames": 256
        })
    );
    Ok(())
}
