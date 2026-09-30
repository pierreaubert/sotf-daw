//! Preserved public audio controls for the AUD141 multiway correction.

// Rust guideline compliant 2026-02-21
use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_plugin_crossover::{CrossoverPlugin, PerChannelOpMode};
use std::path::{Path, PathBuf};

const SAMPLE_RATE: u32 = 48_000;
const FRAMES: usize = 12_288;

fn input_signal() -> Vec<f32> {
    let mut input = Vec::with_capacity(FRAMES * 2);
    for frame in 0..FRAMES {
        let time = frame as f64 / f64::from(SAMPLE_RATE);
        let left = 0.31 * (std::f64::consts::TAU * 223.0 * time).sin()
            + 0.17 * (std::f64::consts::TAU * 907.0 * time + 0.2).sin()
            + 0.09 * (std::f64::consts::TAU * 3_109.0 * time + 0.4).sin();
        let right = 0.23 * (std::f64::consts::TAU * 317.0 * time + 0.6).sin()
            + 0.19 * (std::f64::consts::TAU * 1_211.0 * time + 0.1).sin()
            + 0.11 * (std::f64::consts::TAU * 4_013.0 * time + 0.3).sin();
        input.push(left as f32);
        input.push(right as f32);
    }
    input
}

fn render_case(name: &str) -> (String, Vec<f32>, Vec<f32>) {
    let (mut plugin, settings) = match name {
        "two-way-lr24-stereo" => (
            CrossoverPlugin::new(2, "LR24", 1_250.0, "both").unwrap(),
            "type=LR24;frequency=1250;output=both".to_owned(),
        ),
        "per-channel-lr24-stereo" => (
            CrossoverPlugin::new_per_channel(
                "LR24",
                vec![900.0, 2_100.0],
                vec![PerChannelOpMode::Lowpass, PerChannelOpMode::Highpass],
            )
            .unwrap(),
            "type=LR24;channels=[900:lowpass,2100:highpass]".to_owned(),
        ),
        "four-way-fir-stereo" => (
            CrossoverPlugin::new_multiway(2, "FIR", 700.0, "both", &[1_800.0, 5_000.0]).unwrap(),
            "type=FIR;frequencies=[700,1800,5000];output=both".to_owned(),
        ),
        "four-way-lr24-final-high-stereo" => (
            CrossoverPlugin::new_multiway(2, "LR24", 700.0, "high", &[1_800.0, 5_000.0]).unwrap(),
            "type=LR24;frequencies=[700,1800,5000];output=high".to_owned(),
        ),
        other => panic!("unknown AUD141 baseline case {other}"),
    };
    plugin.initialize(SAMPLE_RATE).unwrap();
    let input = input_signal();
    let output_channels = plugin.output_channels();
    let mut output = vec![f32::NAN; FRAMES * output_channels];
    let processed = plugin
        .process(
            &input,
            &mut output,
            &ProcessContext::new(SAMPLE_RATE, FRAMES),
        )
        .unwrap();
    assert_eq!(processed, FRAMES);
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!(
        output.iter().any(|sample| sample.abs() > 1.0e-4),
        "{name} baseline must contain meaningful processed audio"
    );
    let description = format!(
        "case={name};settings={settings};sample_rate={SAMPLE_RATE};frames={FRAMES};input_channels=2;output_channels={output_channels}"
    );
    (description, input, output)
}

fn baseline_directory() -> PathBuf {
    std::env::var_os("SOTF_AUDIT_BASELINE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../../audit/artifacts/aud142-preedit/aud141")
        })
}

fn capture_directory() -> PathBuf {
    PathBuf::from(
        std::env::var_os("SOTF_AUDIT_BASELINE_DIR")
            .expect("set SOTF_AUDIT_BASELINE_DIR to an explicit capture directory"),
    )
}

fn write_f32_le(path: &Path, samples: &[f32]) {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(samples));
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    std::fs::write(path, bytes).unwrap();
}

fn read_f32_le(path: &Path) -> Vec<f32> {
    let bytes = std::fs::read(path).unwrap();
    assert_eq!(bytes.len() % std::mem::size_of::<f32>(), 0);
    let (chunks, remainder) = bytes.as_chunks::<4>();
    assert!(remainder.is_empty());
    chunks
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect()
}

const CASES: [&str; 4] = [
    "two-way-lr24-stereo",
    "per-channel-lr24-stereo",
    "four-way-fir-stereo",
    "four-way-lr24-final-high-stereo",
];

#[test]
#[ignore = "manual AUD141 pre-edit source and sample capture"]
fn capture_aud141_pre_edit_control_audio() {
    let directory = capture_directory();
    std::fs::create_dir_all(&directory).unwrap();
    for name in CASES {
        let (description, input, output) = render_case(name);
        write_f32_le(&directory.join(format!("{name}-input-f32le.bin")), &input);
        write_f32_le(&directory.join(format!("{name}-output-f32le.bin")), &output);
        std::fs::write(
            directory.join(format!("{name}-metadata.txt")),
            format!("{description}\n"),
        )
        .unwrap();
    }
}

#[test]
fn replay_aud141_pre_edit_control_audio_bit_exactly() {
    let directory = baseline_directory();
    for name in CASES {
        let (description, input, output) = render_case(name);
        let metadata =
            std::fs::read_to_string(directory.join(format!("{name}-metadata.txt"))).unwrap();
        assert_eq!(metadata, format!("{description}\n"));
        assert_eq!(
            input,
            read_f32_le(&directory.join(format!("{name}-input-f32le.bin")))
        );
        assert_eq!(
            output,
            read_f32_le(&directory.join(format!("{name}-output-f32le.bin"))),
            "pre-edit public output changed for {name}"
        );
    }
}
