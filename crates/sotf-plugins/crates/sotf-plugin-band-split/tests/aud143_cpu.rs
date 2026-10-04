// Rust guideline compliant 2026-02-21

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::{Plugin, ProcessContext};
use sotf_plugin_band_split::BandSplitPlugin;
use std::hint::black_box;
use std::time::Instant;

const SAMPLE_RATE: u32 = 48_000;
const WARMUP_FRAMES: usize = 8_192;
const MEASURE_FRAMES: usize = 32_768;
const MEASUREMENT_SAMPLES: usize = 9;
const SETUP_INSTANCES_PER_SAMPLE: usize = 64;
const BLOCK_SIZES: [usize; 3] = [32, 512, 2_048];

struct Workload {
    name: &'static str,
    input_channels: usize,
    crossover_type: &'static str,
    cutoffs_hz: &'static [f64],
}

const WORKLOADS: [Workload; 5] = [
    Workload {
        name: "stereo-2band-lr24",
        input_channels: 2,
        crossover_type: "LR24",
        cutoffs_hz: &[1_000.0],
    },
    Workload {
        name: "stereo-4band-lr24",
        input_channels: 2,
        crossover_type: "LR24",
        cutoffs_hz: &[250.0, 1_000.0, 4_000.0],
    },
    Workload {
        name: "stereo-2band-lr48",
        input_channels: 2,
        crossover_type: "LR48",
        cutoffs_hz: &[1_000.0],
    },
    Workload {
        name: "stereo-4band-lr48",
        input_channels: 2,
        crossover_type: "LR48",
        cutoffs_hz: &[250.0, 1_000.0, 4_000.0],
    },
    Workload {
        name: "12ch-4band-lr48",
        input_channels: 12,
        crossover_type: "LR48",
        cutoffs_hz: &[250.0, 1_000.0, 4_000.0],
    },
];

struct ControlEvent {
    relative_frame: usize,
    parameter_id: ParameterId,
    value: ParameterValue,
}

struct Segment {
    start_frame: usize,
    frames: usize,
    context: ProcessContext<'static>,
    event_index: Option<usize>,
}

#[test]
fn aud143_controlled_cpu_comparison() {
    let Ok(variant) = std::env::var("AUD143_VARIANT") else {
        eprintln!("AUD143_VARIANT unset; controlled CPU comparison skipped");
        return;
    };

    let phase_compensated = match variant.as_str() {
        "recovered-legacy" | "current-legacy" => false,
        "current-phase" => true,
        _ => panic!("unsupported AUD143_VARIANT={variant}"),
    };

    println!(
        "AUD143_RUN,variant={variant},sample_rate={SAMPLE_RATE},warmup_frames={WARMUP_FRAMES},measured_frames={MEASURE_FRAMES},samples={MEASUREMENT_SAMPLES}"
    );

    for workload in &WORKLOADS {
        measure_setup(workload, &variant, phase_compensated);
        measure_processing(workload, &variant, phase_compensated);
    }
}

fn create_plugin(workload: &Workload, phase_compensated: bool) -> Box<dyn Plugin> {
    let mut plugin = BandSplitPlugin::new_multiband(
        workload.input_channels,
        workload.cutoffs_hz,
        workload.crossover_type,
    )
    .expect("valid AUD143 benchmark workload");

    if phase_compensated {
        plugin
            .set_parameter(
                ParameterId::from("recombination_mode"),
                ParameterValue::Int(1),
            )
            .expect("current source must support PhaseCompensated setup");
    }

    plugin
        .initialize(f64::from(SAMPLE_RATE))
        .expect("benchmark plugin initialization");
    Box::new(plugin)
}

fn measure_setup(workload: &Workload, variant: &str, phase_compensated: bool) {
    let mut setup_ns_per_instance = Vec::with_capacity(MEASUREMENT_SAMPLES);

    for _ in 0..MEASUREMENT_SAMPLES {
        let mut plugins = Vec::with_capacity(SETUP_INSTANCES_PER_SAMPLE);
        let start = Instant::now();
        for _ in 0..SETUP_INSTANCES_PER_SAMPLE {
            plugins.push(create_plugin(workload, phase_compensated));
        }
        let elapsed = start.elapsed();
        black_box(&plugins);
        let ns_per_instance = elapsed.as_nanos() / SETUP_INSTANCES_PER_SAMPLE as u128;
        setup_ns_per_instance.push(ns_per_instance);
        drop(plugins);
    }

    for (sample_index, &elapsed_ns) in setup_ns_per_instance.iter().enumerate() {
        println!(
            "AUD143_SETUP_SAMPLE,variant={variant},workload={},sample={sample_index},ns_per_instance={elapsed_ns}",
            workload.name
        );
    }
    let (minimum, median, maximum) = summary_stats(&mut setup_ns_per_instance);
    println!(
        "AUD143_SETUP,variant={variant},workload={},slope={},bands={},channels={},samples={},instances_per_sample={},min_ns={minimum},median_ns={median},max_ns={maximum}",
        workload.name,
        workload.crossover_type,
        workload.cutoffs_hz.len() + 1,
        workload.input_channels,
        MEASUREMENT_SAMPLES,
        SETUP_INSTANCES_PER_SAMPLE
    );
}

fn measure_processing(workload: &Workload, variant: &str, phase_compensated: bool) {
    let bands = workload.cutoffs_hz.len() + 1;
    let output_channels = workload.input_channels * bands;
    let total_frames = WARMUP_FRAMES + MEASURE_FRAMES;
    let input = make_input(workload.input_channels, total_frames);
    let events = control_events(workload);

    for block_frames in BLOCK_SIZES {
        let warmup_segments = build_segments(0, WARMUP_FRAMES, block_frames, &[]);
        let steady_segments = build_segments(WARMUP_FRAMES, total_frames, block_frames, &[]);
        let automation_segments =
            build_segments(WARMUP_FRAMES, total_frames, block_frames, &events);

        for (scenario, segments) in [
            ("steady", steady_segments.as_slice()),
            ("automation", automation_segments.as_slice()),
        ] {
            let mut elapsed_ns = Vec::with_capacity(MEASUREMENT_SAMPLES);
            let mut last_peak = 0.0_f32;
            let mut last_rms = 0.0_f64;
            let mut last_checksum = 0.0_f64;

            for _sample_index in 0..MEASUREMENT_SAMPLES {
                let mut plugin = create_plugin(workload, phase_compensated);
                let mut output = vec![0.0_f32; total_frames * output_channels];
                process_segments(
                    plugin.as_mut(),
                    &input,
                    &mut output,
                    workload.input_channels,
                    output_channels,
                    &warmup_segments,
                    &[],
                );

                let start = Instant::now();
                process_segments(
                    plugin.as_mut(),
                    &input,
                    &mut output,
                    workload.input_channels,
                    output_channels,
                    segments,
                    &events,
                );
                elapsed_ns.push(start.elapsed().as_nanos());

                (last_peak, last_rms, last_checksum) = validate_output(&output);
                black_box(last_checksum);
            }

            for (sample_index, &elapsed_ns) in elapsed_ns.iter().enumerate() {
                println!(
                    "AUD143_PROCESS_SAMPLE,variant={variant},workload={},scenario={scenario},block_frames={block_frames},sample={sample_index},elapsed_ns={elapsed_ns}",
                    workload.name
                );
            }
            let (minimum, median, maximum) = summary_stats(&mut elapsed_ns);
            println!(
                "AUD143_PROCESS,variant={variant},workload={},slope={},bands={},channels={},scenario={scenario},block_frames={block_frames},calls={},samples={},frames_per_sample={},min_ns={minimum},median_ns={median},max_ns={maximum},median_ns_per_frame={:.3},median_ns_per_call={:.1},peak={last_peak:.8},rms={last_rms:.8},checksum={last_checksum:.8}",
                workload.name,
                workload.crossover_type,
                bands,
                workload.input_channels,
                segments.len(),
                MEASUREMENT_SAMPLES,
                MEASURE_FRAMES,
                median as f64 / MEASURE_FRAMES as f64,
                median as f64 / segments.len() as f64
            );
        }
    }
}

fn control_events(workload: &Workload) -> Vec<ControlEvent> {
    let (first, second) = if workload.cutoffs_hz.len() == 1 {
        (1_500.0_f32, 800.0_f32)
    } else {
        (300.0_f32, 400.0_f32)
    };

    vec![
        ControlEvent {
            relative_frame: 701,
            parameter_id: ParameterId::from("frequency"),
            value: ParameterValue::Float(first),
        },
        ControlEvent {
            relative_frame: 9_113,
            parameter_id: ParameterId::from("frequency"),
            value: ParameterValue::Float(second),
        },
    ]
}

fn build_segments(
    start_frame: usize,
    end_frame: usize,
    max_block_frames: usize,
    events: &[ControlEvent],
) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut current_frame = start_frame;
    let mut event_index = 0;

    while current_frame < end_frame {
        let event_at_start = events
            .get(event_index)
            .filter(|event| start_frame + event.relative_frame == current_frame)
            .map(|_| {
                let index = event_index;
                event_index += 1;
                index
            });

        let next_event_frame = events
            .get(event_index)
            .map(|event| start_frame + event.relative_frame)
            .unwrap_or(end_frame);
        let frames = max_block_frames
            .min(end_frame - current_frame)
            .min(next_event_frame.saturating_sub(current_frame));
        assert!(frames > 0, "AUD143 event must align to a segment boundary");

        segments.push(Segment {
            start_frame: current_frame,
            frames,
            context: ProcessContext::new(SAMPLE_RATE, frames)
                .with_sample_position(current_frame as u64),
            event_index: event_at_start,
        });
        current_frame += frames;
    }

    segments
}

fn process_segments(
    plugin: &mut dyn Plugin,
    input: &[f32],
    output: &mut [f32],
    input_channels: usize,
    output_channels: usize,
    segments: &[Segment],
    events: &[ControlEvent],
) {
    for segment in segments {
        if let Some(event_index) = segment.event_index {
            let event = &events[event_index];
            plugin
                .set_parameter(event.parameter_id.clone(), event.value.clone())
                .expect("sample-timed frequency event");
        }

        let input_start = segment.start_frame * input_channels;
        let input_end = input_start + segment.frames * input_channels;
        let output_start = segment.start_frame * output_channels;
        let output_end = output_start + segment.frames * output_channels;
        let processed = plugin
            .process(
                black_box(&input[input_start..input_end]),
                black_box(&mut output[output_start..output_end]),
                &segment.context,
            )
            .expect("BandSplit callback");
        assert_eq!(processed, segment.frames);
    }
}

fn make_input(channels: usize, frames: usize) -> Vec<f32> {
    let mut input = Vec::with_capacity(channels * frames);
    for frame in 0..frames {
        let time_seconds = frame as f64 / f64::from(SAMPLE_RATE);
        for channel in 0..channels {
            let channel_offset = channel as f64 * 0.193;
            let lower = (std::f64::consts::TAU * (143.0 + channel as f64 * 17.0) * time_seconds
                + channel_offset)
                .sin();
            let upper = (std::f64::consts::TAU * (3_271.0 + channel as f64 * 29.0) * time_seconds
                + channel_offset * 1.7)
                .sin();
            let modulation = (std::f64::consts::TAU * 7.0 * time_seconds + channel_offset).sin();
            input.push((0.19 * lower + 0.07 * upper + 0.013 * modulation) as f32);
        }
    }
    input
}

fn validate_output(output: &[f32]) -> (f32, f64, f64) {
    let mut peak = 0.0_f32;
    let mut power = 0.0_f64;
    let mut checksum = 0.0_f64;
    for (index, &sample) in output.iter().enumerate() {
        assert!(
            sample.is_finite(),
            "non-finite AUD143 output sample {index}"
        );
        peak = peak.max(sample.abs());
        power += f64::from(sample) * f64::from(sample);
        checksum += f64::from(sample) * ((index % 251 + 1) as f64);
    }
    let rms = (power / output.len() as f64).sqrt();
    assert!(peak > 1.0e-5 && rms > 1.0e-5, "AUD143 output is trivial");
    (peak, rms, checksum)
}

fn summary_stats(samples: &mut [u128]) -> (u128, u128, u128) {
    samples.sort_unstable();
    let last = samples.len() - 1;
    let median = samples[last / 2];
    (samples[0], median, samples[last])
}
