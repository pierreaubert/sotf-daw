//! Scalar-versus-span stream profiling with symmetric oracle handling.

// Rust guideline compliant 2026-02-21
use super::*;
use std::hint::black_box;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

include!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../../../audit/artifacts/remaining-convolution-eq-span-20261024/scalar-stream-baseline.rs.txt"
));

const FRAMES: usize = 128;
#[derive(serde::Deserialize)]
struct Case {
    name: String,
    rate: u32,
    channels: usize,
    ir_channels: usize,
    frames: usize,
    matrix: bool,
    nupc: bool,
    head: bool,
    control: bool,
    warm: usize,
    calls: usize,
}
struct Fixture(PathBuf);
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn fixture(case: &Case, index: usize) -> Fixture {
    // Match the full QA harness: the RIFF decoder refuses 32-channel PCM,
    // while AIFF preserves the same supported channel/sample geometry.
    let aiff = case.ir_channels == 32;
    let path = std::env::temp_dir().join(format!(
        "sotf-span-paired-{}-{index}.{}",
        std::process::id(),
        if aiff { "aiff" } else { "wav" }
    ));
    let size = u32::try_from(case.frames * case.ir_channels * 2).unwrap();
    let mut bytes = Vec::with_capacity(54 + size as usize);
    if aiff {
        bytes.extend(b"FORM");
        bytes.extend((46 + size).to_be_bytes());
        bytes.extend(b"AIFFCOMM");
        bytes.extend(18_u32.to_be_bytes());
        bytes.extend((case.ir_channels as u16).to_be_bytes());
        bytes.extend((case.frames as u32).to_be_bytes());
        bytes.extend(16_u16.to_be_bytes());
        let exponent = 0x400e_u16 + (case.rate / 48_000).ilog2() as u16;
        bytes.extend(exponent.to_be_bytes());
        bytes.extend([0xbb, 0x80, 0, 0, 0, 0, 0, 0]);
        bytes.extend(b"SSND");
        bytes.extend((8 + size).to_be_bytes());
        bytes.extend([0; 8]);
    } else {
        bytes.extend(b"RIFF");
        bytes.extend((36 + size).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16_u32.to_le_bytes());
        bytes.extend(1_u16.to_le_bytes());
        bytes.extend((case.ir_channels as u16).to_le_bytes());
        bytes.extend(case.rate.to_le_bytes());
        bytes.extend((case.rate * case.ir_channels as u32 * 2).to_le_bytes());
        bytes.extend((case.ir_channels as u16 * 2).to_le_bytes());
        bytes.extend(16_u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend(size.to_le_bytes());
    }
    for frame in 0..case.frames {
        for channel in 0..case.ir_channels {
            let sample = (((frame * 17 + channel * 29) % 101) as i16 - 50) * 64;
            let sample = if sample == 0 { 1 } else { sample };
            bytes.extend(if aiff {
                sample.to_be_bytes()
            } else {
                sample.to_le_bytes()
            });
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

fn plugin(case: &Case, file: Option<&Fixture>) -> ConvolutionPlugin {
    let mut result = if let Some(file) = file {
        ConvolutionPlugin::from_params_with_routing(
            case.channels,
            case.rate,
            ConvolutionPluginParams {
                ir_file: file.0.to_string_lossy().into_owned(),
                use_nupc: case.nupc,
                zero_latency_head: case.head,
                head_taps: 257,
                mix: 0.37,
                gain_db: -3.0,
            },
            case.matrix,
        )
        .unwrap()
    } else {
        ConvolutionPlugin::new(case.channels, case.rate)
    };
    if case.control {
        result.use_nupc = false;
    }
    assert_eq!(result.state.as_ref().is_some(), case.frames > 0);
    if case.nupc {
        assert_eq!(
            result.nupc_engines.len(),
            if case.matrix { 4 } else { case.channels }
        );
        assert_eq!(result.nupc_engines[0].head_taps() > 0, case.head);
    } else {
        assert!(result.nupc_engines.is_empty());
    }
    result
}
type Stream = fn(&mut ConvolutionPlugin, &mut [f32], &ProcessContext<'_>) -> PluginResult<usize>;
fn warm(
    plugin: &mut ConvolutionPlugin,
    render: Stream,
    case: &Case,
    inputs: &[Vec<f32>],
    output: &mut [f32],
    context: &ProcessContext<'_>,
) {
    for block in 0..case.warm {
        output.copy_from_slice(&inputs[block % inputs.len()]);
        assert_eq!(render(plugin, output, context).unwrap(), FRAMES);
        black_box(&*output);
    }
}

#[test]
#[ignore = "Explicit setup-only validation of all paired profiling fixtures"]
fn paired_span_all_fixtures_load() {
    let plan: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../audit/artifacts/remaining-convolution-eq-span-20261024/paired-plan.json"
    )))
    .unwrap();
    let cases: Vec<Case> = serde_json::from_value(plan["cases"].clone()).unwrap();
    assert_eq!(cases.len(), 48);
    for (index, case) in cases.iter().enumerate() {
        let file = (case.frames > 0).then(|| fixture(case, index));
        let mut instance = plugin(case, file.as_ref());
        let mut output = vec![0.125; case.channels * FRAMES];
        let context = ProcessContext::new(case.rate, FRAMES);
        assert_eq!(
            instance.process_stream(&mut output, &context).unwrap(),
            FRAMES
        );
        assert!(output.iter().all(|sample| sample.is_finite()));
        println!("SPAN_SETUP {}", case.name);
    }
}

#[test]
#[ignore = "Explicit scalar/span ABBA stream diagnostic; never a CI timing gate"]
fn paired_span_stream_cost_abba() {
    let plan: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../../../audit/artifacts/remaining-convolution-eq-span-20261024/paired-plan.json"
    )))
    .unwrap();
    let cases: Vec<Case> = serde_json::from_value(plan["cases"].clone()).unwrap();
    assert_eq!(cases.len(), 48);
    for (case_index, case) in cases.iter().enumerate() {
        let file = if case.frames == 0 {
            None
        } else {
            Some(fixture(case, case_index))
        };
        let inputs: Vec<Vec<f32>> = (0..17)
            .map(|k| {
                (0..case.channels * FRAMES)
                    .map(|i| (((i * 13 + k * 31) % 127) as f32 - 63.5) / 128.0)
                    .collect()
            })
            .collect();
        let context = ProcessContext::new(case.rate, FRAMES);
        let samples = case.channels * FRAMES;
        let mut reference = vec![0.0; case.calls * samples];
        // Compute expected output before all timed trials. Every timed arm
        // reads the same oracle and performs identical output handling.
        {
            let mut oracle = plugin(case, file.as_ref());
            let mut output = vec![0.0; samples];
            warm(
                &mut oracle,
                ConvolutionPlugin::process_stream_scalar_span_baseline,
                case,
                &inputs,
                &mut output,
                &context,
            );
            for block in 0..case.calls {
                output.copy_from_slice(&inputs[(case.warm + block) % inputs.len()]);
                assert_eq!(
                    oracle
                        .process_stream_scalar_span_baseline(&mut output, &context)
                        .unwrap(),
                    FRAMES
                );
                reference[block * samples..(block + 1) * samples].copy_from_slice(&output);
            }
        }
        for (trial, candidate) in [false, true, true, false].into_iter().enumerate() {
            let mut instance = plugin(case, file.as_ref());
            // Common controls deliberately execute one identical function in
            // both arms, rather than separately compiled equivalent prefixes.
            let render: Stream = if case.control || !candidate {
                ConvolutionPlugin::process_stream_scalar_span_baseline
            } else {
                ConvolutionPlugin::process_stream
            };
            let function = if case.control {
                "common_scalar"
            } else if candidate {
                "span"
            } else {
                "scalar"
            };
            let mut output = vec![0.0; samples];
            let mut durations = Vec::with_capacity(case.calls);
            warm(&mut instance, render, case, &inputs, &mut output, &context);
            let mut maximum_error = 0.0_f32;
            let mut checksum = 0.0_f64;
            for block in 0..case.calls {
                output.copy_from_slice(&inputs[(case.warm + block) % inputs.len()]);
                let start = Instant::now();
                let rendered = render(&mut instance, &mut output, &context);
                let elapsed = start.elapsed().as_nanos() as u64;
                black_box(&output);
                durations.push(elapsed);
                assert_eq!(rendered.unwrap(), FRAMES);
                for (&sample, &expected) in output
                    .iter()
                    .zip(&reference[block * samples..(block + 1) * samples])
                {
                    assert!(sample.is_finite());
                    maximum_error = maximum_error.max((sample - expected).abs());
                    checksum += f64::from(sample);
                }
            }
            assert!(maximum_error <= 2e-5, "{}: {maximum_error}", case.name);
            if case.control || !candidate {
                assert_eq!(maximum_error, 0.0);
            }
            let total_ns: u64 = durations.iter().sum();
            println!(
                "SPAN_PAIRED {}",
                serde_json::json!({"case":case.name,"rate":case.rate,"trial":trial,
                "candidate":candidate,"control":case.control,"function":function,"warm":case.warm,
                "calls":case.calls,"total_ns":total_ns,"max_error":maximum_error,"checksum":checksum,
                "durations_ns":durations})
            );
        }
    }
}
