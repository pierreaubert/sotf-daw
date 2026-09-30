//! Durable pre-edit Crossover audio and runtime-parameter evidence for AUD142.

// Rust guideline compliant 2026-02-21
use sotf_host::parameters::Parameter;
use sotf_host::plugin::{Plugin, PluginDrainResult, ProcessContext, TailLength};
use sotf_plugin_crossover::{CrossoverPlugin, PerChannelOpMode};
use std::path::{Path, PathBuf};

const SAMPLE_RATE: u32 = 48_000;
const FRAMES: usize = 12_288;
const INPUT_CHANNELS: usize = 2;
const FIR_TAPS: usize = 1_025;
const FIR_SPLITS: usize = 3;
const FIR_TAIL_FRAMES: usize = FIR_SPLITS * (FIR_TAPS - 1);

struct RenderedCase {
    description: String,
    input: Vec<f32>,
    process: Vec<f32>,
    tail: Vec<f32>,
    metadata: String,
}

fn input_signal() -> Vec<f32> {
    let mut input = Vec::with_capacity(FRAMES * INPUT_CHANNELS);
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

fn make_case(name: &str) -> (CrossoverPlugin, String, usize, bool) {
    match name {
        "three-way-lr24-both-stereo" => (
            CrossoverPlugin::new_multiway(2, "LR24", 700.0, "both", &[1_800.0]).unwrap(),
            "type=LR24;frequencies=[700,1800];output=both".to_owned(),
            6,
            false,
        ),
        "four-way-lr24-both-stereo" => (
            CrossoverPlugin::new_multiway(2, "LR24", 700.0, "both", &[1_800.0, 5_000.0]).unwrap(),
            "type=LR24;frequencies=[700,1800,5000];output=both".to_owned(),
            8,
            false,
        ),
        "four-way-lr24-low-stereo" => (
            CrossoverPlugin::new_multiway(2, "LR24", 700.0, "low", &[1_800.0, 5_000.0]).unwrap(),
            "type=LR24;frequencies=[700,1800,5000];output=low".to_owned(),
            2,
            false,
        ),
        "four-way-fir-both-eof-stereo" => (
            CrossoverPlugin::from_params(
                2,
                &sotf_plugin_crossover::CrossoverPluginParams {
                    crossover_type: "FIR".into(),
                    frequency: 700.0,
                    extra_frequencies: vec![1_800.0, 5_000.0],
                    output: "both".into(),
                    fir_taps: Some(FIR_TAPS),
                    channel_frequencies_hz: Vec::new(),
                    channel_modes: Some(Vec::new()),
                    topology: None,
                },
            )
            .unwrap(),
            format!("type=FIR;frequencies=[700,1800,5000];output=both;fir_taps={FIR_TAPS}"),
            8,
            true,
        ),
        "per-channel-lr24-stereo" => (
            CrossoverPlugin::new_per_channel(
                "LR24",
                vec![900.0, 2_100.0],
                vec![PerChannelOpMode::Lowpass, PerChannelOpMode::Highpass],
            )
            .unwrap(),
            "type=LR24;channels=[900:lowpass,2100:highpass]".to_owned(),
            2,
            false,
        ),
        "two-way-lr24-both-stereo" => (
            CrossoverPlugin::new(2, "LR24", 1_250.0, "both").unwrap(),
            "type=LR24;frequency=1250;output=both".to_owned(),
            4,
            false,
        ),
        "four-way-lr24-high-stereo" => (
            CrossoverPlugin::new_multiway(2, "LR24", 700.0, "high", &[1_800.0, 5_000.0]).unwrap(),
            "type=LR24;frequencies=[700,1800,5000];output=high".to_owned(),
            2,
            false,
        ),
        other => panic!("unknown AUD142 baseline case {other}"),
    }
}

fn parameter_metadata(parameters: &[Parameter], plugin: &dyn Plugin) -> String {
    let mut lines = Vec::with_capacity(parameters.len() + 1);
    lines.push(format!("runtime_parameter_count={}", parameters.len()));
    for (index, parameter) in parameters.iter().enumerate() {
        let current = plugin.get_parameter(&parameter.id);
        lines.push(format!(
            "index={index};id={};name={};unit={};default={:?};min={:?};max={:?};logarithmic={};update_mode={:?};current={current:?}",
            parameter.id,
            parameter.name,
            parameter.unit,
            parameter.default_value,
            parameter.min_value,
            parameter.max_value,
            parameter.logarithmic,
            parameter.update_mode,
        ));
    }
    lines.join("\n") + "\n"
}

fn render_case(name: &str) -> RenderedCase {
    let (mut plugin, settings, expected_output_channels, has_fir_tail) = make_case(name);
    plugin.initialize(SAMPLE_RATE).unwrap();
    assert_eq!(plugin.output_channels(), expected_output_channels);

    let input = input_signal();
    let mut process = vec![f32::NAN; FRAMES * expected_output_channels];
    assert_eq!(
        plugin
            .process(
                &input,
                &mut process,
                &ProcessContext::new(SAMPLE_RATE, FRAMES),
            )
            .unwrap(),
        FRAMES
    );
    assert!(process.iter().all(|sample| sample.is_finite()));
    assert!(process.iter().any(|sample| sample.abs() > 1.0e-4));

    let tail = if has_fir_tail {
        assert_eq!(
            plugin.tail_length(),
            TailLength::Finite(FIR_TAIL_FRAMES as u64)
        );
        let mut tail = Vec::new();
        for _ in 0..4_096 {
            let mut buffer = vec![f32::NAN; 257 * expected_output_channels];
            let result = plugin
                .drain(&mut buffer, &ProcessContext::new(SAMPLE_RATE, 0))
                .unwrap();
            assert!(result.frames <= 257);
            assert!(
                buffer[..result.frames * expected_output_channels]
                    .iter()
                    .all(|sample| sample.is_finite())
            );
            tail.extend_from_slice(&buffer[..result.frames * expected_output_channels]);
            if result.complete {
                break;
            }
        }
        assert_eq!(
            plugin
                .drain(&mut [], &ProcessContext::new(SAMPLE_RATE, 0))
                .unwrap(),
            PluginDrainResult::COMPLETE
        );
        assert_eq!(tail.len(), FIR_TAIL_FRAMES * expected_output_channels);
        tail
    } else {
        assert!(matches!(plugin.tail_length(), TailLength::Unknown));
        Vec::new()
    };

    let runtime_parameters = plugin.parameters();
    let metadata = parameter_metadata(&runtime_parameters, &plugin);
    let description = format!(
        "case={name};settings={settings};sample_rate={SAMPLE_RATE};frames={FRAMES};input_channels={INPUT_CHANNELS};output_channels={expected_output_channels};tail_frames={}",
        tail.len() / expected_output_channels
    );

    let active_lanes = expected_output_channels;
    for lane in 0..active_lanes {
        let lane_peak = process
            .chunks_exact(expected_output_channels)
            .map(|frame| frame[lane].abs())
            .fold(0.0_f32, f32::max);
        assert!(lane_peak > 1.0e-4, "{name} output lane {lane} is silent");
    }

    RenderedCase {
        description,
        input,
        process,
        tail,
        metadata,
    }
}

fn capture_directory() -> PathBuf {
    PathBuf::from(
        std::env::var_os("SOTF_AUD142_CAPTURE_DIR")
            .expect("set SOTF_AUD142_CAPTURE_DIR to an explicit pre-edit capture directory"),
    )
}

fn baseline_directory() -> PathBuf {
    std::env::var_os("SOTF_AUD142_BASELINE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../../../audit/artifacts/aud142-preedit/crossover")
        })
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
    let (chunks, remainder) = bytes.as_chunks::<4>();
    assert!(remainder.is_empty());
    chunks
        .iter()
        .map(|chunk| f32::from_le_bytes(*chunk))
        .collect()
}

fn structural_mode_metadata_for_current_route(pre_edit_metadata: &str) -> String {
    let mut adjusted_lines = Vec::new();
    for line in pre_edit_metadata.lines() {
        if line.contains(";id=mode;") {
            assert!(
                line.contains("update_mode=Realtime"),
                "preserved pre-edit mode metadata must retain its recorded update mode"
            );
            adjusted_lines.push(line.replace("update_mode=Realtime", "update_mode=Structural"));
        } else {
            adjusted_lines.push(line.to_owned());
        }
    }
    adjusted_lines.join("\n") + "\n"
}

fn assert_current_mode_is_structural(metadata: &str) {
    for line in metadata.lines().filter(|line| line.contains(";id=mode;")) {
        assert!(
            line.contains("update_mode=Structural"),
            "current mode metadata must reflect width-changing structural routing: {line}"
        );
    }
}

const CASES: [&str; 7] = [
    "two-way-lr24-both-stereo",
    "three-way-lr24-both-stereo",
    "four-way-lr24-both-stereo",
    "four-way-lr24-low-stereo",
    "four-way-lr24-high-stereo",
    "per-channel-lr24-stereo",
    "four-way-fir-both-eof-stereo",
];

#[test]
#[ignore = "manual AUD142 pre-edit audio and runtime metadata capture"]
fn capture_aud142_pre_edit_audio_and_runtime_metadata() {
    let directory = capture_directory();
    std::fs::create_dir_all(&directory).unwrap();
    let common_input = input_signal();
    write_f32_le(&directory.join("input-f32le.bin"), &common_input);
    for name in CASES {
        let rendered = render_case(name);
        assert_eq!(rendered.input, common_input);
        write_f32_le(
            &directory.join(format!("{name}-process-f32le.bin")),
            &rendered.process,
        );
        write_f32_le(
            &directory.join(format!("{name}-tail-f32le.bin")),
            &rendered.tail,
        );
        std::fs::write(
            directory.join(format!("{name}-metadata.txt")),
            format!("{}\n{}", rendered.description, rendered.metadata),
        )
        .unwrap();
    }
}

#[test]
fn replay_aud142_pre_edit_audio_and_runtime_metadata_bit_exactly() {
    let directory = baseline_directory();
    let expected_input = read_f32_le(&directory.join("input-f32le.bin"));
    assert_eq!(expected_input.len(), FRAMES * INPUT_CHANNELS);
    for name in CASES {
        let rendered = render_case(name);
        assert_eq!(rendered.input, expected_input);
        let metadata =
            std::fs::read_to_string(directory.join(format!("{name}-metadata.txt"))).unwrap();
        let current_metadata = format!("{}\n{}", rendered.description, rendered.metadata);
        assert_current_mode_is_structural(&current_metadata);
        assert_eq!(
            structural_mode_metadata_for_current_route(&metadata),
            current_metadata,
            "only the intentional legacy mode update-mode change is permitted"
        );
        assert_eq!(
            rendered.process,
            read_f32_le(&directory.join(format!("{name}-process-f32le.bin"))),
            "pre-edit process audio changed for {name}"
        );
        assert_eq!(
            rendered.tail,
            read_f32_le(&directory.join(format!("{name}-tail-f32le.bin"))),
            "pre-edit EOF audio changed for {name}"
        );
    }
}
