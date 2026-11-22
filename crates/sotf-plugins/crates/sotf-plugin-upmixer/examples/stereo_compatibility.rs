//! Render deterministic stereo cases for comparison with an unchanged build.

use sotf_host::{ParameterValue, Plugin, ProcessContext};
use sotf_plugin_upmixer::{UpmixerPlugin, UpmixerPluginParams};
use std::io::{BufWriter, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args().nth(1).ok_or("expected an output file")?;
    let mut writer = BufWriter::new(std::fs::File::create(path)?);
    let mut cases = 0;
    for rate in [48_000_u32, 96_000] {
        for layout in ["5.1", "7.1.4", "9.1.6"] {
            for fft in [512, 2048] {
                for hr in [false, true] {
                    for auto_gain in [false, true] {
                        for bypass in [false, true] {
                            let params: UpmixerPluginParams = serde_json::from_value(
                                serde_json::json!({
                                    "fft_size": fft, "speaker_config": layout, "enable_hr_direct": hr,
                                    "auto_gain_enabled": auto_gain, "bypass_all_processing": bypass
                                }),
                            )?;
                            let mut plugin = UpmixerPlugin::from_params(params);
                            plugin.initialize(f64::from(rate))?;
                            let channels = plugin.output_channels();
                            let mut seed = 0x17ad_4c31_u32;
                            let mut position = 0_usize;
                            let mut call = 0_usize;
                            let patterns = [257, 17, 512, 31, 1024, 3];
                            while position < 12_288 {
                                let frames = patterns[call % patterns.len()].min(12_288 - position);
                                let mut input = Vec::with_capacity(frames * 2);
                                for frame in position..position + frames {
                                    seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                                    let noise = (seed as f32 / u32::MAX as f32 - 0.5) * 0.06;
                                    let phase = frame as f32 / rate as f32;
                                    let transient = if frame % 1709 == 0 { 0.25 } else { 0.0 };
                                    input.push(
                                        0.16 * (std::f32::consts::TAU * 347.0 * phase).sin()
                                            + noise
                                            + transient,
                                    );
                                    input.push(
                                        0.14 * (std::f32::consts::TAU * 719.0 * phase).cos()
                                            - noise * 0.7,
                                    );
                                }
                                if call % 7 == 1 {
                                    let gain = if (call / 7) % 2 == 0 { 0.4 } else { 1.1 };
                                    plugin.set_parameter(
                                        "height_gain".into(),
                                        ParameterValue::Float(gain),
                                    )?;
                                }
                                let mut output = vec![0.0; frames * channels];
                                plugin.process(
                                    &input,
                                    &mut output,
                                    &ProcessContext::new(rate, frames),
                                )?;
                                for sample in output {
                                    if !sample.is_finite() {
                                        return Err("non-finite output".into());
                                    }
                                    writer.write_all(&sample.to_le_bytes())?;
                                }
                                position += frames;
                                call += 1;
                            }
                            cases += 1;
                        }
                    }
                }
            }
        }
    }
    writer.flush()?;
    println!("rendered {cases} stereo cases");
    Ok(())
}
