// Rust guideline compliant 2026-02-21
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::PathBuf;

use sotf_host::{Plugin, ProcessContext, TailLength};
use sotf_plugin_ambisonics::{AmbisonicsDecoderConfig, AmbisonicsDecoderPlugin};

const SAMPLE_RATE: u32 = 48_000;
const PROGRAM_FRAMES: usize = 2_049;
const SILENCE_FRAMES: usize = 2_048;
const PARTITIONS: [usize; 6] = [1, 17, 137, 256, 613, 1_024];
const TARGET_LAYOUTS: [&str; 8] = [
    "5.1", "7.1", "5.1.2", "5.1.4", "7.1.2", "7.1.4", "9.1.4", "9.1.6",
];
const ALGORITHMS: [(&str, &str); 2] = [("mode_matching", "mm"), ("allrad", "allrad")];

fn program_signal(input_channels: usize) -> Vec<f32> {
    let mut samples = vec![0.0; PROGRAM_FRAMES * input_channels];
    for frame in 0..PROGRAM_FRAMES {
        for channel in 0..input_channels {
            let mut value = (frame as u32 + 1).wrapping_mul(0x9e37_79b9)
                ^ (channel as u32 + 3).wrapping_mul(0x85eb_ca6b);
            value ^= value >> 16;
            value = value.wrapping_mul(0x7feb_352d);
            value ^= value >> 15;
            let centered = ((value >> 8) & 0xffff) as i32 - 32_768;
            samples[frame * input_channels + channel] = centered as f32 / 32_768.0 * 0.03;
        }
    }

    for channel in 0..input_channels {
        samples[(PROGRAM_FRAMES - 1) * input_channels + channel] +=
            0.12 * (channel + 1) as f32 / input_channels as f32;
    }
    samples
}

fn process_partitioned(
    plugin: &mut AmbisonicsDecoderPlugin,
    input: &[f32],
    frames: usize,
    first_sample: u64,
) -> Vec<f32> {
    let input_channels = plugin.input_channels();
    let output_channels = plugin.output_channels();
    assert_eq!(input.len(), frames * input_channels);

    let mut output = vec![0.0; frames * output_channels];
    let mut frame = 0;
    let mut partition_index = 0;
    while frame < frames {
        let block_frames = PARTITIONS[partition_index % PARTITIONS.len()].min(frames - frame);
        let input_start = frame * input_channels;
        let input_end = (frame + block_frames) * input_channels;
        let output_start = frame * output_channels;
        let output_end = (frame + block_frames) * output_channels;
        let context = ProcessContext::new(SAMPLE_RATE, block_frames)
            .with_sample_position(first_sample + frame as u64);
        assert_eq!(
            plugin
                .process(
                    &input[input_start..input_end],
                    &mut output[output_start..output_end],
                    &context,
                )
                .unwrap(),
            block_frames
        );
        frame += block_frames;
        partition_index += 1;
    }
    output
}

fn render_program_and_silence(plugin: &mut AmbisonicsDecoderPlugin) -> Vec<f32> {
    let input_channels = plugin.input_channels();
    let output_channels = plugin.output_channels();
    let program = program_signal(input_channels);
    let silence = vec![0.0; SILENCE_FRAMES * input_channels];
    let mut output = process_partitioned(plugin, &program, PROGRAM_FRAMES, 0);
    output.extend(process_partitioned(
        plugin,
        &silence,
        SILENCE_FRAMES,
        PROGRAM_FRAMES as u64,
    ));
    assert_eq!(
        output.len(),
        (PROGRAM_FRAMES + SILENCE_FRAMES) * output_channels
    );
    assert!(output.iter().all(|sample| sample.is_finite()));
    output
}

fn fnv1a64(samples: &[f32]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for sample in samples {
        for byte in sample.to_bits().to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    hash
}

#[test]
#[ignore = "captures the pre-edit AUD133 audio arrays and lower-order digests"]
fn capture_pre_edit_lower_order_audio_arrays() {
    let artifact_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/audit-artifacts/aud133-preedit-9302797/audio-arrays");
    fs::create_dir_all(&artifact_dir).unwrap();

    let mut case_count = 0;
    for order in 1..=3 {
        for layout in TARGET_LAYOUTS {
            for (algorithm, algorithm_file) in ALGORITHMS {
                for max_re_weighting in [false, true] {
                    for dual_band in [false, true] {
                        let config = AmbisonicsDecoderConfig {
                            order,
                            target_layout: layout.to_owned(),
                            max_re_weighting,
                            dual_band,
                            algorithm: algorithm.to_owned(),
                        };
                        let mut plugin = AmbisonicsDecoderPlugin::new(&config).unwrap();
                        plugin.initialize(SAMPLE_RATE).unwrap();

                        let expected_tail = if dual_band {
                            TailLength::Unknown
                        } else {
                            TailLength::Finite(0)
                        };
                        assert_eq!(plugin.tail_length(), expected_tail);
                        assert_eq!(plugin.drain_call_bound().map(|bound| bound.get()), Some(1));

                        let output = render_program_and_silence(&mut plugin);
                        let mut drain_sentinel = [1.25_f32, -6.5_f32];
                        let drain_result = plugin
                            .drain(&mut drain_sentinel, &ProcessContext::new(SAMPLE_RATE, 0))
                            .unwrap();
                        assert_eq!(drain_result.frames, 0);
                        assert!(drain_result.complete);
                        assert_eq!(drain_sentinel, [1.25, -6.5]);

                        plugin.reset();
                        assert_eq!(
                            render_program_and_silence(&mut plugin),
                            output,
                            "reset replay differs for order={order} layout={layout} algorithm={algorithm} max_re={max_re_weighting} dual_band={dual_band}"
                        );

                        let file_name = format!(
                            "order{order}_{layout}_{algorithm_file}_maxre{}_dual{}.f32le",
                            u8::from(max_re_weighting),
                            u8::from(dual_band)
                        );
                        let mut writer =
                            BufWriter::new(File::create(artifact_dir.join(file_name)).unwrap());
                        for sample in &output {
                            writer.write_all(&sample.to_le_bytes()).unwrap();
                        }
                        writer.flush().unwrap();

                        println!(
                            "AUD133_BASELINE order={order} layout={layout} algorithm={algorithm} max_re={} dual_band={} input_frames={} silence_frames={} output_channels={} fnv1a64={:016x} tail={expected_tail:?} drain_frames={} drain_complete={}",
                            u8::from(max_re_weighting),
                            u8::from(dual_band),
                            PROGRAM_FRAMES,
                            SILENCE_FRAMES,
                            plugin.output_channels(),
                            fnv1a64(&output),
                            drain_result.frames,
                            drain_result.complete,
                        );
                        case_count += 1;
                    }
                }
            }
        }
    }
    assert_eq!(case_count, 192);
}

#[test]
fn lower_order_output_fingerprints_match_pre_edit_capture() {
    let mut expected = Vec::new();
    for line in include_str!("data/aud133_preedit_output_fingerprints.txt").lines() {
        if line.starts_with('#') || line.trim().is_empty() {
            continue;
        }
        let fields: Vec<_> = line.split_ascii_whitespace().collect();
        assert_eq!(fields.len(), 7, "invalid baseline fixture row: {line}");
        expected.push((
            fields[0].parse::<usize>().unwrap(),
            fields[1],
            fields[2],
            fields[3] == "1",
            fields[4] == "1",
            fields[5].parse::<usize>().unwrap(),
            u64::from_str_radix(fields[6], 16).unwrap(),
        ));
    }
    assert_eq!(expected.len(), 192);

    let mut case_index = 0;
    for order in 1..=3 {
        for layout in TARGET_LAYOUTS {
            for (algorithm, _) in ALGORITHMS {
                for max_re_weighting in [false, true] {
                    for dual_band in [false, true] {
                        let config = AmbisonicsDecoderConfig {
                            order,
                            target_layout: layout.to_owned(),
                            max_re_weighting,
                            dual_band,
                            algorithm: algorithm.to_owned(),
                        };
                        let mut plugin = AmbisonicsDecoderPlugin::new(&config).unwrap();
                        plugin.initialize(SAMPLE_RATE).unwrap();
                        let output = render_program_and_silence(&mut plugin);
                        let fixture = expected[case_index];
                        assert_eq!(fixture.0, order);
                        assert_eq!(fixture.1, layout);
                        assert_eq!(fixture.2, algorithm);
                        assert_eq!(fixture.3, max_re_weighting);
                        assert_eq!(fixture.4, dual_band);
                        assert_eq!(fixture.5, plugin.output_channels());
                        assert_eq!(
                            fnv1a64(&output),
                            fixture.6,
                            "pre-edit output changed for order={order}, layout={layout}, algorithm={algorithm}, max_re={max_re_weighting}, dual_band={dual_band}"
                        );
                        case_index += 1;
                    }
                }
            }
        }
    }
}
