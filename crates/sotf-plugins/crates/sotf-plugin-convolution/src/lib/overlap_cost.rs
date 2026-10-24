//! Frozen UPC stream bodies with paired callback timing and unchanged controls.

// Rust guideline compliant 2026-02-21
use super::*;
use std::hint::black_box;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../audit/artifacts/remaining-convolution-eq-20261024/legacy-upc-stream-cost.rs.txt"
));
include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../audit/artifacts/remaining-convolution-eq-20261024/candidate-upc-stream-cost.rs.txt"
));

const FRAMES: usize = 128;
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn fixture(rate: u32, frames: usize, channels: usize) -> Fixture {
    let path = std::env::temp_dir().join(format!(
        "sotf-ola-paired-{}-{rate}-{channels}.wav",
        std::process::id()
    ));
    let size = (frames * channels * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + size as usize);
    bytes.extend(b"RIFF");
    bytes.extend((36 + size).to_le_bytes());
    bytes.extend(b"WAVEfmt ");
    bytes.extend(16_u32.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend((channels as u16).to_le_bytes());
    bytes.extend(rate.to_le_bytes());
    bytes.extend((rate * channels as u32 * 2).to_le_bytes());
    bytes.extend((channels as u16 * 2).to_le_bytes());
    bytes.extend(16_u16.to_le_bytes());
    bytes.extend(b"data");
    bytes.extend(size.to_le_bytes());
    for frame in 0..frames {
        for channel in 0..channels {
            let sample = (((frame * 17 + channel * 29) % 101) as i16 - 50) * 64;
            bytes.extend(sample.to_le_bytes());
        }
    }
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .unwrap();
    file.write_all(&bytes).unwrap();
    Fixture(path)
}

#[test]
#[ignore = "Explicit frozen-stream ABBA wall diagnostic; never a CI timing gate"]
fn paired_overlap_callback_cost_abba() {
    // Moderate dense IRs isolate output/mix overhead without claiming maximum
    // admission capacity; separate unchanged full-load gates retain that scope.
    for rate in [48_000, 96_000, 192_000] {
        for (name, channels, ir_channels, length) in [
            ("mono2", 2, 1, 65_537),
            ("full12", 12, 12, 65_537),
            ("mono32", 32, 1, 65_537),
            ("control12", 12, 1, 0),
            ("control32", 32, 1, 0),
        ] {
            let file = if length == 0 {
                None
            } else {
                Some(fixture(rate, length, ir_channels))
            };
            let warm = (length + PARTITION_SIZE + 37).div_ceil(FRAMES).max(256);
            let calls = 1024;
            let inputs: Vec<Vec<f32>> = (0..17)
                .map(|k| {
                    (0..channels * FRAMES)
                        .map(|i| (((i * 13 + k * 31) % 127) as f32 - 63.5) / 128.0)
                        .collect()
                })
                .collect();
            let mut reference = vec![0.0; calls * channels * FRAMES];
            for (number, candidate) in [false, true, true, false].into_iter().enumerate() {
                let mut plugin = if let Some(file) = &file {
                    ConvolutionPlugin::from_params(
                        channels,
                        rate,
                        ConvolutionPluginParams {
                            ir_file: file.0.to_string_lossy().into_owned(),
                            use_nupc: false,
                            mix: 0.37,
                            gain_db: -3.0,
                            ..Default::default()
                        },
                    )
                    .unwrap()
                } else {
                    ConvolutionPlugin::new(channels, rate)
                };
                plugin.use_nupc = false;
                assert!(plugin.nupc_engines.is_empty());
                assert_eq!(plugin.state.as_ref().is_some(), length != 0);
                let context = ProcessContext::new(rate, FRAMES);
                let mut output = vec![0.0; channels * FRAMES];
                let mut durations = Vec::with_capacity(calls);
                let mut render = |output: &mut [f32]| {
                    if candidate {
                        plugin
                            .process_stream_candidate_for_cost(output, &context)
                            .unwrap();
                    } else {
                        plugin
                            .process_stream_legacy_for_cost(output, &context)
                            .unwrap();
                    }
                };
                for block in 0..warm {
                    output.copy_from_slice(&inputs[block % inputs.len()]);
                    render(&mut output);
                    black_box(&output);
                }
                let mut error = 0.0_f32;
                let mut checksum = 0.0_f64;
                for block in 0..calls {
                    output.copy_from_slice(&inputs[(warm + block) % inputs.len()]);
                    let start = Instant::now();
                    render(&mut output);
                    let elapsed = start.elapsed().as_nanos() as u64;
                    black_box(&output);
                    durations.push(elapsed);
                    let saved = &mut reference[block * output.len()..(block + 1) * output.len()];
                    if number == 0 {
                        saved.copy_from_slice(&output);
                    }
                    for (&sample, &expected) in output.iter().zip(saved.iter()) {
                        assert!(sample.is_finite());
                        error = error.max((sample - expected).abs());
                        checksum += f64::from(sample);
                    }
                }
                assert!(error <= 2e-5, "{name} {rate}: {error}");
                if name.starts_with("control") || !candidate {
                    assert_eq!(error, 0.0);
                }
                let total_ns: u64 = durations.iter().sum();
                println!(
                    "PAIRED {{\"backend\":\"upc\",\"case\":\"{name}\",\"rate\":{rate},\"trial\":{number},\"candidate\":{candidate},\"warm\":{warm},\"calls\":{calls},\"total_ns\":{total_ns},\"max_error\":{error},\"checksum\":{checksum},\"durations_ns\":{durations:?}}}"
                );
            }
        }
    }
}
