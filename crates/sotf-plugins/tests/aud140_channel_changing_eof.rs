//! AUD140 public host evidence for finite streams through channel-changing nodes.

// Rust guideline compliant 2026-09-29
use sotf_host::host::{DawHost, GraphEdge};
use sotf_host::plugin::{Plugin, PluginDrainResult, PluginInfo, ProcessContext, TailLength};
use sotf_plugin_ambisonics::{AmbisonicsDecoderConfig, AmbisonicsDecoderPlugin};
use std::env;
use std::fs;
use std::num::NonZeroU64;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const SAMPLE_RATE: u32 = 48_000;
const ORDER7_INPUT_CHANNELS: usize = 64;
const ORDER7_OUTPUT_CHANNELS: usize = 16;
const FIR_TAIL_FRAMES: usize = 2;
// `DawHost::build` currently prepares graph scratch for 8192 frames by 32
// channels. The oversized fixtures below must exceed that prepared storage.
const PREPARED_GRAPH_SCRATCH_SAMPLES: usize = 8192 * 32;

fn ambisonics_config(order: usize, target_layout: &str) -> AmbisonicsDecoderConfig {
    AmbisonicsDecoderConfig {
        order,
        target_layout: target_layout.to_owned(),
        max_re_weighting: true,
        dual_band: false,
        algorithm: "mode_matching".to_owned(),
    }
}

fn patterned_input(frames: usize, channels: usize) -> Vec<f32> {
    (0..frames * channels)
        .map(|sample| {
            let value = ((sample * 17 + sample / channels * 3 + 19) % 127) as i32 - 63;
            value as f32 / 128.0
        })
        .collect()
}

fn finite_tail_input() -> Vec<f32> {
    let mut input = patterned_input(97, ORDER7_INPUT_CHANNELS);
    let final_frame = 96 * ORDER7_INPUT_CHANNELS;
    input[final_frame] = 0.75;
    input[final_frame + 1..final_frame + ORDER7_INPUT_CHANNELS].fill(0.0);
    input
}

fn process_ambisonics(input: &[f32], config: &AmbisonicsDecoderConfig) -> Vec<f32> {
    let mut plugin = AmbisonicsDecoderPlugin::new(config).unwrap();
    plugin.initialize(SAMPLE_RATE).unwrap();
    let frames = input.len() / plugin.input_channels();
    assert_eq!(input.len(), frames * plugin.input_channels());
    let mut output = vec![f32::NAN; frames * plugin.output_channels()];
    assert_eq!(
        plugin
            .process(
                input,
                &mut output,
                &ProcessContext::new(SAMPLE_RATE, frames),
            )
            .unwrap(),
        frames
    );
    output
}

fn process_host_ambisonics(input: &[f32], config: &AmbisonicsDecoderConfig) -> Vec<f32> {
    let input_channels = (config.order + 1) * (config.order + 1);
    let plugin = AmbisonicsDecoderPlugin::new(config).unwrap();
    let output_channels = plugin.output_channels();
    let frames = input.len() / input_channels;
    let mut host = DawHost::new(input_channels, SAMPLE_RATE);
    host.add_plugin(Box::new(plugin)).unwrap();
    host.build().unwrap();
    assert_eq!(host.output_channels(), output_channels);
    let mut output = vec![f32::NAN; frames * output_channels];
    assert_eq!(host.process(input, &mut output).unwrap(), frames);
    output
}

#[derive(Default)]
struct DrainCalls {
    processes: AtomicUsize,
    begins: AtomicUsize,
    drains: AtomicUsize,
}

struct FiniteFirProducer {
    sample_rate: u32,
    channels: usize,
    previous: Vec<f64>,
    two_back: Vec<f64>,
    drain_remaining: usize,
    drain_capacity_frames: usize,
    identity_frame_geometry: bool,
    output_sample_rate: Option<u32>,
    draining: bool,
    calls: Arc<DrainCalls>,
}

impl FiniteFirProducer {
    fn new(channels: usize, calls: Arc<DrainCalls>) -> Self {
        Self::with_drain_capacity(channels, calls, 1)
    }

    fn with_drain_capacity(
        channels: usize,
        calls: Arc<DrainCalls>,
        drain_capacity_frames: usize,
    ) -> Self {
        Self {
            sample_rate: SAMPLE_RATE,
            channels,
            previous: vec![0.0; channels],
            two_back: vec![0.0; channels],
            drain_remaining: 0,
            drain_capacity_frames,
            identity_frame_geometry: true,
            output_sample_rate: None,
            draining: false,
            calls,
        }
    }

    fn with_identity_frame_geometry(mut self, identity: bool) -> Self {
        self.identity_frame_geometry = identity;
        self
    }

    fn with_output_sample_rate(mut self, sample_rate: Option<u32>) -> Self {
        self.output_sample_rate = sample_rate;
        self
    }

    fn filter_sample(&mut self, channel: usize, input: f32) -> f32 {
        let output =
            f64::from(input) + 0.5 * self.previous[channel] + 0.25 * self.two_back[channel];
        self.two_back[channel] = self.previous[channel];
        self.previous[channel] = f64::from(input);
        output as f32
    }
}

impl Plugin for FiniteFirProducer {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("AUD140 finite FIR producer", "test", "SotF")
    }

    fn input_channels(&self) -> usize {
        self.channels
    }

    fn output_channels(&self) -> usize {
        self.channels
    }

    fn parameters(&self) -> Vec<sotf_host::parameters::Parameter> {
        Vec::new()
    }

    fn set_parameter(
        &mut self,
        _id: sotf_host::parameters::ParameterId,
        _value: sotf_host::parameters::ParameterValue,
    ) -> Result<(), String> {
        Err("AUD140 finite FIR producer has no parameters".to_owned())
    }

    fn get_parameter(
        &self,
        _id: &sotf_host::parameters::ParameterId,
    ) -> Option<sotf_host::parameters::ParameterValue> {
        None
    }

    fn initialize(&mut self, sample_rate: u32) -> Result<(), String> {
        if sample_rate == 0 {
            return Err("AUD140 producer sample rate must be positive".to_owned());
        }
        self.sample_rate = sample_rate;
        self.reset();
        Ok(())
    }

    fn reset(&mut self) {
        self.previous.fill(0.0);
        self.two_back.fill(0.0);
        self.drain_remaining = 0;
        self.draining = false;
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        let samples = context
            .num_frames
            .checked_mul(self.channels)
            .ok_or_else(|| "AUD140 producer block size overflow".to_owned())?;
        if context.sample_rate != self.sample_rate
            || input.len() != samples
            || output.len() != samples
        {
            return Err("AUD140 producer received invalid process geometry".to_owned());
        }
        if self.draining {
            return Err("AUD140 producer requires reset after drain begins".to_owned());
        }
        for frame in 0..context.num_frames {
            for channel in 0..self.channels {
                let index = frame * self.channels + channel;
                output[index] = self.filter_sample(channel, input[index]);
            }
        }
        self.calls.processes.fetch_add(1, Ordering::Relaxed);
        Ok(context.num_frames)
    }

    fn tail_length(&self) -> TailLength {
        TailLength::Finite(FIR_TAIL_FRAMES as u64)
    }

    fn latency_samples(&self) -> usize {
        0
    }

    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        input_frames
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        self.identity_frame_geometry
    }

    fn output_sample_rate(&self, input_rate: u32) -> u32 {
        self.output_sample_rate.unwrap_or(input_rate)
    }

    fn drain_output_frames_max(&self) -> usize {
        self.drain_capacity_frames
    }

    fn drain_call_bound(&self) -> Option<NonZeroU64> {
        NonZeroU64::new(FIR_TAIL_FRAMES as u64)
    }

    fn begin_drain(&mut self, context: &ProcessContext) -> Result<(), String> {
        if context.sample_rate != self.sample_rate || context.num_frames != 0 || self.draining {
            return Err("AUD140 producer received invalid drain preparation".to_owned());
        }
        self.calls.begins.fetch_add(1, Ordering::Relaxed);
        self.drain_remaining = FIR_TAIL_FRAMES;
        self.draining = true;
        Ok(())
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        if context.sample_rate != self.sample_rate || context.num_frames != 0 {
            return Err("AUD140 producer received invalid drain context".to_owned());
        }
        if !self.draining || self.drain_remaining == 0 {
            return Ok(PluginDrainResult::COMPLETE);
        }
        if output.len() < self.channels {
            return Err("AUD140 producer needs one full output frame".to_owned());
        }
        for (channel, sample) in output[..self.channels].iter_mut().enumerate() {
            *sample = self.filter_sample(channel, 0.0);
        }
        self.drain_remaining -= 1;
        self.calls.drains.fetch_add(1, Ordering::Relaxed);
        Ok(PluginDrainResult {
            frames: 1,
            complete: self.drain_remaining == 0,
        })
    }
}

fn make_order7_finite_tail_host() -> (DawHost, Arc<DrainCalls>) {
    make_order7_finite_tail_host_with_contract(true, None)
}

fn make_order7_finite_tail_host_with_contract(
    identity_frame_geometry: bool,
    output_sample_rate: Option<u32>,
) -> (DawHost, Arc<DrainCalls>) {
    let mut host = DawHost::new(ORDER7_INPUT_CHANNELS, SAMPLE_RATE);
    let calls = Arc::new(DrainCalls::default());
    let producer = FiniteFirProducer::new(ORDER7_INPUT_CHANNELS, Arc::clone(&calls))
        .with_identity_frame_geometry(identity_frame_geometry)
        .with_output_sample_rate(output_sample_rate);
    host.add_plugin(Box::new(producer)).unwrap();
    host.add_plugin(Box::new(
        AmbisonicsDecoderPlugin::new(&ambisonics_config(7, "9.1.6")).unwrap(),
    ))
    .unwrap();
    host.build().unwrap();
    (host, calls)
}

fn finite_fir_reference(input: &[f32], channels: usize) -> Vec<f32> {
    assert_eq!(input.len() % channels, 0);
    let input_frames = input.len() / channels;
    let output_frames = input_frames + FIR_TAIL_FRAMES;
    let mut previous = vec![0.0_f64; channels];
    let mut two_back = vec![0.0_f64; channels];
    let mut output = vec![0.0_f32; output_frames * channels];
    for frame in 0..output_frames {
        for channel in 0..channels {
            let input_sample = if frame < input_frames {
                f64::from(input[frame * channels + channel])
            } else {
                0.0
            };
            let expected = input_sample + 0.5 * previous[channel] + 0.25 * two_back[channel];
            output[frame * channels + channel] = expected as f32;
            two_back[channel] = previous[channel];
            previous[channel] = input_sample;
        }
    }
    output
}

fn assert_close(actual: &[f32], expected: &[f32], tolerance: f32) {
    assert_eq!(actual.len(), expected.len());
    let peak_error = actual
        .iter()
        .zip(expected)
        .map(|(&actual, &expected)| (actual - expected).abs())
        .fold(0.0_f32, f32::max);
    assert!(
        peak_error <= tolerance,
        "peak error {peak_error} > {tolerance}"
    );
}

fn ordinary_audio_cases() -> Vec<(String, Vec<f32>)> {
    let mut cases = Vec::new();
    for (name, order, layout, frames) in [
        ("order1_4_to_6", 1, "5.1", 97),
        ("order3_16_to_12", 3, "7.1.4", 97),
        ("order7_64_to_16", 7, "9.1.6", 97),
    ] {
        let channels = (order + 1) * (order + 1);
        let input = patterned_input(frames, channels);
        let config = ambisonics_config(order, layout);
        let output = process_host_ambisonics(&input, &config);
        let expected = process_ambisonics(&input, &config);
        assert_close(&output, &expected, 1e-6);
        assert!(output.iter().any(|sample| sample.abs() > 1e-5));
        cases.push((name.to_owned(), output));
    }

    let input = finite_tail_input();
    let (mut host, _) = make_order7_finite_tail_host();
    let mut output = vec![f32::NAN; 97 * ORDER7_OUTPUT_CHANNELS];
    assert_eq!(host.process(&input, &mut output).unwrap(), 97);
    let filtered = finite_fir_reference(&input, ORDER7_INPUT_CHANNELS);
    let filtered_process = &filtered[..input.len()];
    let expected = process_ambisonics(filtered_process, &ambisonics_config(7, "9.1.6"));
    assert_close(&output, &expected, 1e-6);
    cases.push(("finite_fir_64_to_16_process".to_owned(), output));
    cases
}

fn write_baseline_vector(directory: &Path, name: &str, samples: &[f32]) {
    let mut bytes = Vec::with_capacity(std::mem::size_of_val(samples));
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }
    fs::write(directory.join(format!("{name}.f32le")), bytes).unwrap();
}

fn read_baseline_vector(directory: &Path, name: &str) -> Vec<f32> {
    let bytes = fs::read(directory.join(format!("{name}.f32le")))
        .unwrap_or_else(|error| panic!("could not read AUD140 baseline vector {name}: {error}"));
    assert!(bytes.len().is_multiple_of(std::mem::size_of::<f32>()));
    let (samples, remainder) = bytes.as_chunks::<{ std::mem::size_of::<f32>() }>();
    assert!(remainder.is_empty());
    samples
        .iter()
        .map(|sample| f32::from_le_bytes(*sample))
        .collect()
}

fn baseline_directory() -> PathBuf {
    PathBuf::from(
        env::var_os("SOTF_AUDIT_BASELINE_DIR")
            .expect("set SOTF_AUDIT_BASELINE_DIR to the AUD140 baseline directory"),
    )
}

#[test]
fn finite_zero_tail_channel_changing_ambisonics_routes_complete() {
    for (order, layout, input_channels, output_channels) in [
        (1, "5.1", 4, 6),
        (3, "7.1.4", 16, 12),
        (7, "9.1.6", ORDER7_INPUT_CHANNELS, ORDER7_OUTPUT_CHANNELS),
    ] {
        let config = ambisonics_config(order, layout);
        let plugin = AmbisonicsDecoderPlugin::new(&config).unwrap();
        assert_eq!(plugin.input_channels(), input_channels);
        assert_eq!(plugin.output_channels(), output_channels);
        assert_eq!(plugin.tail_length(), TailLength::Finite(0));

        let input = patterned_input(97, input_channels);
        let expected = process_ambisonics(&input, &config);
        assert!(expected.iter().any(|sample| sample.abs() > 1e-5));

        let mut host = DawHost::new(input_channels, SAMPLE_RATE);
        host.add_plugin(Box::new(plugin)).unwrap();
        host.build().unwrap();
        assert_eq!(host.output_channels(), output_channels);
        let mut host_output = vec![f32::NAN; 97 * output_channels];
        assert_eq!(host.process(&input, &mut host_output).unwrap(), 97);
        assert_close(&host_output, &expected, 1e-6);

        let result = host.drain(&mut []).unwrap_or_else(|error| {
            panic!("zero-tail order-{order} {input_channels}→{output_channels} route rejected EOF: {error}")
        });
        assert_eq!(result.frames, 0);
        assert!(result.complete);
    }
}

fn assert_oversized_channel_changing_drain_is_rejected_before_begin(
    order: usize,
    layout: &str,
    input_channels: usize,
    output_channels: usize,
    capacity_frames: usize,
) {
    let calls = Arc::new(DrainCalls::default());
    let input = patterned_input(1, input_channels);
    let config = ambisonics_config(order, layout);
    let mut host = DawHost::new(input_channels, SAMPLE_RATE);
    host.add_plugin(Box::new(FiniteFirProducer::with_drain_capacity(
        input_channels,
        Arc::clone(&calls),
        capacity_frames,
    )))
    .unwrap();
    host.add_plugin(Box::new(AmbisonicsDecoderPlugin::new(&config).unwrap()))
        .unwrap();
    host.build().unwrap();
    assert_eq!(host.drain_output_frames_max(), capacity_frames);

    let mut process_output = vec![f32::NAN; output_channels];
    assert_eq!(host.process(&input, &mut process_output).unwrap(), 1);
    let mut drain_output = vec![0.25; capacity_frames * output_channels];
    let error = host
        .drain(&mut drain_output)
        .expect_err("unprepared intermediate drain extent must be rejected");
    assert!(
        error.contains("scratch"),
        "unexpected preflight error: {error}"
    );
    assert!(drain_output.iter().all(|sample| *sample == 0.25));
    assert_eq!(calls.begins.load(Ordering::Relaxed), 0);
    assert_eq!(calls.drains.load(Ordering::Relaxed), 0);
}

#[test]
fn drain_preflight_rejects_expansion_and_contraction_scratch_extents() {
    // The 4→6 expansion itself is the largest intermediate extent.
    let expansion_capacity = PREPARED_GRAPH_SCRATCH_SAMPLES / 6 + 1;
    assert_oversized_channel_changing_drain_is_rejected_before_begin(
        1,
        "5.1",
        4,
        6,
        expansion_capacity,
    );

    // The source producer's 64-channel output exceeds scratch before the
    // 64→16 contraction reduces the extent.
    let contraction_capacity = PREPARED_GRAPH_SCRATCH_SAMPLES / ORDER7_INPUT_CHANNELS + 1;
    assert_oversized_channel_changing_drain_is_rejected_before_begin(
        7,
        "9.1.6",
        ORDER7_INPUT_CHANNELS,
        ORDER7_OUTPUT_CHANNELS,
        contraction_capacity,
    );
}

#[test]
fn caller_capacity_and_alignment_rejection_preserve_channel_changing_tail() {
    let input = finite_tail_input();
    let expected = finite_fir_reference(&input, ORDER7_INPUT_CHANNELS);
    let expected_tail = &expected[input.len()..];
    let calls = Arc::new(DrainCalls::default());
    let mut host = DawHost::new(ORDER7_INPUT_CHANNELS, SAMPLE_RATE);
    host.add_plugin(Box::new(FiniteFirProducer::new(
        ORDER7_INPUT_CHANNELS,
        Arc::clone(&calls),
    )))
    .unwrap();
    host.add_plugin(Box::new(
        AmbisonicsDecoderPlugin::new(&ambisonics_config(7, "9.1.6")).unwrap(),
    ))
    .unwrap();
    host.build().unwrap();

    let mut process_output = vec![f32::NAN; 97 * ORDER7_OUTPUT_CHANNELS];
    assert_eq!(host.process(&input, &mut process_output).unwrap(), 97);
    assert_eq!(host.drain_output_frames_max(), 1);

    let error = host.drain(&mut []).unwrap_err();
    assert!(error.contains("too small"), "unexpected error: {error}");
    let mut misaligned = vec![0.125; ORDER7_OUTPUT_CHANNELS - 1];
    let error = host.drain(&mut misaligned).unwrap_err();
    assert!(error.contains("whole frames"), "unexpected error: {error}");
    assert!(misaligned.iter().all(|sample| *sample == 0.125));
    assert_eq!(calls.begins.load(Ordering::Relaxed), 0);
    assert_eq!(calls.drains.load(Ordering::Relaxed), 0);

    let mut actual_tail = Vec::new();
    let mut complete = false;
    for _ in 0..FIR_TAIL_FRAMES + 1 {
        let mut chunk = vec![f32::NAN; ORDER7_OUTPUT_CHANNELS];
        let result = host.drain(&mut chunk).unwrap();
        actual_tail.extend_from_slice(&chunk[..result.frames * ORDER7_OUTPUT_CHANNELS]);
        if result.complete {
            complete = true;
            break;
        }
    }
    assert!(complete);
    let expected_decoded = process_ambisonics(expected_tail, &ambisonics_config(7, "9.1.6"));
    assert_eq!(actual_tail.len(), expected_decoded.len());
    assert_close(&actual_tail, &expected_decoded, 1e-6);
    assert_eq!(calls.begins.load(Ordering::Relaxed), 1);
    assert_eq!(calls.drains.load(Ordering::Relaxed), FIR_TAIL_FRAMES);
}

#[test]
fn finite_producer_tail_crosses_order7_64_to_16_serial_route() {
    let input = finite_tail_input();
    let expected_producer = finite_fir_reference(&input, ORDER7_INPUT_CHANNELS);
    let mut host_output = vec![f32::NAN; 97 * ORDER7_OUTPUT_CHANNELS];
    let (mut host, calls) = make_order7_finite_tail_host();
    assert_eq!(host.output_channels(), ORDER7_OUTPUT_CHANNELS);
    assert_eq!(host.process(&input, &mut host_output).unwrap(), 97);

    let expected_process = process_ambisonics(
        &expected_producer[..input.len()],
        &ambisonics_config(7, "9.1.6"),
    );
    assert_close(&host_output, &expected_process, 1e-6);

    let expected_tail_input = &expected_producer[input.len()..];
    assert_eq!(
        expected_tail_input.len(),
        FIR_TAIL_FRAMES * ORDER7_INPUT_CHANNELS
    );
    let expected_tail = process_ambisonics(expected_tail_input, &ambisonics_config(7, "9.1.6"));
    assert!(expected_tail.iter().any(|sample| sample.abs() > 1e-5));

    let mut final_marker_input = vec![0.0; ORDER7_INPUT_CHANNELS];
    final_marker_input[0] = 0.1875;
    let expected_final_marker =
        process_ambisonics(&final_marker_input, &ambisonics_config(7, "9.1.6"));
    let expected_final_frame =
        &expected_tail[expected_tail.len() - ORDER7_OUTPUT_CHANNELS..expected_tail.len()];
    assert_close(expected_final_frame, &expected_final_marker, 1e-6);
    assert!(
        expected_final_frame
            .iter()
            .any(|sample| sample.abs() > 1e-5)
    );

    let capacity_frames = host.drain_output_frames_max();
    assert_eq!(capacity_frames, 1);
    let mut actual_tail = Vec::new();
    let mut complete = false;
    for _ in 0..FIR_TAIL_FRAMES + 1 {
        let mut chunk = vec![f32::NAN; capacity_frames * ORDER7_OUTPUT_CHANNELS];
        let result = host
            .drain(&mut chunk)
            .unwrap_or_else(|error| panic!("finite 64→64→16 public host drain failed: {error}"));
        assert!(result.frames <= capacity_frames);
        actual_tail.extend_from_slice(&chunk[..result.frames * ORDER7_OUTPUT_CHANNELS]);
        if result.complete {
            complete = true;
            break;
        }
    }
    assert!(
        complete,
        "finite serial route must return a completion receipt"
    );
    assert_eq!(actual_tail.len(), expected_tail.len());
    assert_close(&actual_tail, &expected_tail, 1e-6);
    assert_close(
        &actual_tail[actual_tail.len() - ORDER7_OUTPUT_CHANNELS..],
        expected_final_frame,
        1e-6,
    );
    assert_eq!(calls.begins.load(Ordering::Relaxed), 1);
    assert_eq!(calls.drains.load(Ordering::Relaxed), FIR_TAIL_FRAMES);
}

#[test]
fn finite_producer_tail_expands_intermediate_scratch_from_four_to_six_channels() {
    let channels = 4;
    let output_channels = 6;
    let input = patterned_input(97, channels);
    let expected_producer = finite_fir_reference(&input, channels);
    let expected_full = process_ambisonics(&expected_producer, &ambisonics_config(1, "5.1"));
    let input_output_samples = input.len() / channels * output_channels;

    let calls = Arc::new(DrainCalls::default());
    let mut host = DawHost::new(channels, SAMPLE_RATE);
    host.add_plugin(Box::new(FiniteFirProducer::new(
        channels,
        Arc::clone(&calls),
    )))
    .unwrap();
    host.add_plugin(Box::new(
        AmbisonicsDecoderPlugin::new(&ambisonics_config(1, "5.1")).unwrap(),
    ))
    .unwrap();
    host.build().unwrap();

    let mut process_output = vec![f32::NAN; input_output_samples];
    assert_eq!(host.process(&input, &mut process_output).unwrap(), 97);
    assert_close(
        &process_output,
        &expected_full[..input_output_samples],
        1e-6,
    );

    let expected_tail = &expected_full[input_output_samples..];
    assert_eq!(expected_tail.len(), FIR_TAIL_FRAMES * output_channels);
    assert!(expected_tail.iter().any(|sample| sample.abs() > 1e-5));
    let capacity_frames = host.drain_output_frames_max();
    assert_eq!(capacity_frames, 1);

    let mut actual_tail = Vec::new();
    let mut complete = false;
    for _ in 0..FIR_TAIL_FRAMES + 1 {
        let mut chunk = vec![f32::NAN; capacity_frames * output_channels];
        let result = host.drain(&mut chunk).unwrap();
        actual_tail.extend_from_slice(&chunk[..result.frames * output_channels]);
        if result.complete {
            complete = true;
            break;
        }
    }
    assert!(complete, "expanded finite route must complete EOF");
    assert_eq!(actual_tail.len(), expected_tail.len());
    assert_close(&actual_tail, expected_tail, 1e-6);
    assert_eq!(calls.begins.load(Ordering::Relaxed), 1);
    assert_eq!(calls.drains.load(Ordering::Relaxed), FIR_TAIL_FRAMES);
}

#[test]
fn two_finite_producers_flush_causally_across_width_changing_stage() {
    let input = finite_tail_input();
    let first_fir = finite_fir_reference(&input, ORDER7_INPUT_CHANNELS);
    let decoded = process_ambisonics(&first_fir, &ambisonics_config(7, "9.1.6"));
    let expected_full = finite_fir_reference(&decoded, ORDER7_OUTPUT_CHANNELS);
    let process_samples = 97 * ORDER7_OUTPUT_CHANNELS;
    let first_calls = Arc::new(DrainCalls::default());
    let second_calls = Arc::new(DrainCalls::default());

    let mut host = DawHost::new(ORDER7_INPUT_CHANNELS, SAMPLE_RATE);
    host.add_plugin(Box::new(FiniteFirProducer::new(
        ORDER7_INPUT_CHANNELS,
        Arc::clone(&first_calls),
    )))
    .unwrap();
    host.add_plugin(Box::new(
        AmbisonicsDecoderPlugin::new(&ambisonics_config(7, "9.1.6")).unwrap(),
    ))
    .unwrap();
    host.add_plugin(Box::new(FiniteFirProducer::new(
        ORDER7_OUTPUT_CHANNELS,
        Arc::clone(&second_calls),
    )))
    .unwrap();
    host.build().unwrap();

    let mut process_output = vec![f32::NAN; process_samples];
    assert_eq!(host.process(&input, &mut process_output).unwrap(), 97);
    assert_close(&process_output, &expected_full[..process_samples], 1e-6);

    let expected_tail = &expected_full[process_samples..];
    assert_eq!(
        expected_tail.len(),
        FIR_TAIL_FRAMES * 2 * ORDER7_OUTPUT_CHANNELS
    );
    assert!(expected_tail.iter().any(|sample| sample.abs() > 1e-5));
    let capacity_frames = host.drain_output_frames_max();
    assert_eq!(capacity_frames, 1);

    let mut actual_tail = Vec::new();
    for producer_frame in 0..FIR_TAIL_FRAMES {
        let mut chunk = vec![f32::NAN; capacity_frames * ORDER7_OUTPUT_CHANNELS];
        let result = host.drain(&mut chunk).unwrap();
        assert_eq!(result.frames, 1);
        assert!(!result.complete);
        actual_tail.extend_from_slice(&chunk[..ORDER7_OUTPUT_CHANNELS]);
        assert_eq!(
            first_calls.drains.load(Ordering::Relaxed),
            producer_frame + 1
        );
        assert_eq!(
            second_calls.processes.load(Ordering::Relaxed),
            producer_frame + 2
        );
        assert_eq!(second_calls.begins.load(Ordering::Relaxed), 0);
        assert_eq!(second_calls.drains.load(Ordering::Relaxed), 0);
    }

    let mut complete = false;
    for _ in 0..FIR_TAIL_FRAMES + 1 {
        let mut chunk = vec![f32::NAN; capacity_frames * ORDER7_OUTPUT_CHANNELS];
        let result = host.drain(&mut chunk).unwrap();
        actual_tail.extend_from_slice(&chunk[..result.frames * ORDER7_OUTPUT_CHANNELS]);
        if result.complete {
            complete = true;
            break;
        }
    }
    assert!(complete, "both finite producers must complete EOF");
    assert_eq!(actual_tail.len(), expected_tail.len());
    assert_close(&actual_tail, expected_tail, 1e-6);
    assert_eq!(first_calls.begins.load(Ordering::Relaxed), 1);
    assert_eq!(first_calls.drains.load(Ordering::Relaxed), FIR_TAIL_FRAMES);
    assert_eq!(second_calls.begins.load(Ordering::Relaxed), 1);
    assert_eq!(second_calls.drains.load(Ordering::Relaxed), FIR_TAIL_FRAMES);
}

#[test]
fn width_preserving_bypass_keeps_channel_changing_eof_route_and_audio() {
    let input = finite_tail_input();
    let filtered = finite_fir_reference(&input, ORDER7_INPUT_CHANNELS);
    let expected_full = process_ambisonics(&filtered, &ambisonics_config(7, "9.1.6"));
    let process_samples = 97 * ORDER7_OUTPUT_CHANNELS;
    let source_calls = Arc::new(DrainCalls::default());
    let bypass_calls = Arc::new(DrainCalls::default());

    let mut host = DawHost::new(ORDER7_INPUT_CHANNELS, SAMPLE_RATE);
    host.add_plugin(Box::new(FiniteFirProducer::new(
        ORDER7_INPUT_CHANNELS,
        Arc::clone(&source_calls),
    )))
    .unwrap();
    host.add_plugin(Box::new(FiniteFirProducer::new(
        ORDER7_INPUT_CHANNELS,
        Arc::clone(&bypass_calls),
    )))
    .unwrap();
    host.add_plugin(Box::new(
        AmbisonicsDecoderPlugin::new(&ambisonics_config(7, "9.1.6")).unwrap(),
    ))
    .unwrap();
    host.bypass_plugin(1).unwrap();
    host.build().unwrap();

    let mut process_output = vec![f32::NAN; process_samples];
    assert_eq!(host.process(&input, &mut process_output).unwrap(), 97);
    assert_close(&process_output, &expected_full[..process_samples], 1e-6);
    assert_eq!(bypass_calls.processes.load(Ordering::Relaxed), 0);

    let expected_tail = &expected_full[process_samples..];
    let capacity_frames = host.drain_output_frames_max();
    assert_eq!(capacity_frames, 1);
    let mut actual_tail = Vec::new();
    let mut complete = false;
    for _ in 0..FIR_TAIL_FRAMES + 1 {
        let mut chunk = vec![f32::NAN; capacity_frames * ORDER7_OUTPUT_CHANNELS];
        let result = host.drain(&mut chunk).unwrap();
        actual_tail.extend_from_slice(&chunk[..result.frames * ORDER7_OUTPUT_CHANNELS]);
        if result.complete {
            complete = true;
            break;
        }
    }
    assert!(
        complete,
        "width-preserving bypass must allow EOF completion"
    );
    assert_eq!(actual_tail.len(), expected_tail.len());
    assert_close(&actual_tail, expected_tail, 1e-6);
    assert_eq!(bypass_calls.begins.load(Ordering::Relaxed), 0);
    assert_eq!(bypass_calls.drains.load(Ordering::Relaxed), 0);
}

#[test]
fn ambisonics_identity_geometry_covers_successful_single_and_dual_band_paths() {
    for (dual_band, sample_rates, expected_tail) in [
        (false, vec![1, SAMPLE_RATE], TailLength::Finite(0)),
        (true, vec![1401, SAMPLE_RATE], TailLength::Unknown),
    ] {
        for sample_rate in sample_rates {
            let mut config = ambisonics_config(7, "9.1.6");
            config.dual_band = dual_band;
            let mut plugin = AmbisonicsDecoderPlugin::new(&config).unwrap();
            plugin.initialize(sample_rate).unwrap();
            assert!(plugin.guarantees_identity_frame_geometry());
            assert_eq!(plugin.output_sample_rate(sample_rate), sample_rate);
            assert_eq!(plugin.tail_length(), expected_tail);

            for frames in [0, 1, 19, 257] {
                let input = patterned_input(frames, ORDER7_INPUT_CHANNELS);
                let mut output = vec![f32::NAN; frames * ORDER7_OUTPUT_CHANNELS];
                assert_eq!(
                    plugin
                        .process(
                            &input,
                            &mut output,
                            &ProcessContext::new(sample_rate, frames),
                        )
                        .unwrap(),
                    frames,
                    "dual_band={dual_band}, rate={sample_rate}, frames={frames}"
                );
                assert_eq!(plugin.output_frames_for_input(frames), frames);
                assert!(output.iter().all(|sample| sample.is_finite()));
            }
        }
    }
}

#[test]
fn unknown_dual_band_tail_is_rejected_without_changing_processing_state() {
    let mut config = ambisonics_config(7, "9.1.6");
    config.dual_band = true;
    let mut with_eof_probe = DawHost::new(ORDER7_INPUT_CHANNELS, SAMPLE_RATE);
    let mut uninterrupted = DawHost::new(ORDER7_INPUT_CHANNELS, SAMPLE_RATE);
    for host in [&mut with_eof_probe, &mut uninterrupted] {
        host.add_plugin(Box::new(AmbisonicsDecoderPlugin::new(&config).unwrap()))
            .unwrap();
        host.build().unwrap();
        assert!(host.has_identity_frame_geometry());
    }

    let first_input = patterned_input(79, ORDER7_INPUT_CHANNELS);
    let mut first_output = vec![f32::NAN; 79 * ORDER7_OUTPUT_CHANNELS];
    let mut control_first_output = first_output.clone();
    assert_eq!(
        with_eof_probe
            .process(&first_input, &mut first_output)
            .unwrap(),
        79
    );
    assert_eq!(
        uninterrupted
            .process(&first_input, &mut control_first_output)
            .unwrap(),
        79
    );
    assert_eq!(first_output, control_first_output);

    let error = with_eof_probe.drain(&mut []).unwrap_err();
    assert!(
        error.contains("finite tail"),
        "unexpected rejection: {error}"
    );

    let next_input = patterned_input(23, ORDER7_INPUT_CHANNELS);
    let mut next_output = vec![f32::NAN; 23 * ORDER7_OUTPUT_CHANNELS];
    let mut control_next_output = next_output.clone();
    assert_eq!(
        with_eof_probe
            .process(&next_input, &mut next_output)
            .unwrap(),
        23
    );
    assert_eq!(
        uninterrupted
            .process(&next_input, &mut control_next_output)
            .unwrap(),
        23
    );
    assert_close(&next_output, &control_next_output, 1e-6);
}

fn assert_channel_changing_geometry_refusal_preserves_processing_state(
    identity_frame_geometry: bool,
    output_sample_rate: Option<u32>,
    expected_error: &str,
) {
    let (mut with_drain_attempt, calls) =
        make_order7_finite_tail_host_with_contract(identity_frame_geometry, output_sample_rate);
    let (mut uninterrupted, _) =
        make_order7_finite_tail_host_with_contract(identity_frame_geometry, output_sample_rate);

    let first_input = patterned_input(79, ORDER7_INPUT_CHANNELS);
    let mut first_output = vec![f32::NAN; 79 * ORDER7_OUTPUT_CHANNELS];
    let mut control_first_output = first_output.clone();
    assert_eq!(
        with_drain_attempt
            .process(&first_input, &mut first_output)
            .unwrap(),
        79
    );
    assert_eq!(
        uninterrupted
            .process(&first_input, &mut control_first_output)
            .unwrap(),
        79
    );
    assert_close(&first_output, &control_first_output, 1e-6);

    let mut drain_output = vec![0.0; ORDER7_OUTPUT_CHANNELS];
    let error = with_drain_attempt
        .drain(&mut drain_output)
        .expect_err("unsupported geometry must be rejected before native drain");
    assert!(error.contains(expected_error), "unexpected error: {error}");
    assert_eq!(calls.begins.load(Ordering::Relaxed), 0);
    assert_eq!(calls.drains.load(Ordering::Relaxed), 0);

    let next_input = patterned_input(23, ORDER7_INPUT_CHANNELS);
    let mut next_output = vec![f32::NAN; 23 * ORDER7_OUTPUT_CHANNELS];
    let mut control_next_output = next_output.clone();
    assert_eq!(
        with_drain_attempt
            .process(&next_input, &mut next_output)
            .unwrap(),
        23
    );
    assert_eq!(
        uninterrupted
            .process(&next_input, &mut control_next_output)
            .unwrap(),
        23
    );
    assert_close(&next_output, &control_next_output, 1e-6);
}

#[test]
fn channel_changing_drain_requires_explicit_identity_geometry_before_begin() {
    assert_channel_changing_geometry_refusal_preserves_processing_state(
        false,
        None,
        "explicit identity frame geometry",
    );
}

#[test]
fn channel_changing_drain_rejects_unequal_negotiated_rates_before_begin() {
    assert_channel_changing_geometry_refusal_preserves_processing_state(
        true,
        Some(44_100),
        "host sample rate",
    );
}

#[test]
fn channel_changing_drain_rejects_invalid_adjacent_widths_before_begin() {
    let first_calls = Arc::new(DrainCalls::default());
    let last_calls = Arc::new(DrainCalls::default());
    let mut host = DawHost::new(4, SAMPLE_RATE);
    let source = host
        .add_node(
            "four-channel finite source".to_owned(),
            Box::new(FiniteFirProducer::new(4, Arc::clone(&first_calls))),
        )
        .unwrap();
    let decoder = host
        .add_node(
            "4-to-6 Ambisonics stage".to_owned(),
            Box::new(AmbisonicsDecoderPlugin::new(&ambisonics_config(1, "5.1")).unwrap()),
        )
        .unwrap();
    let incompatible = host
        .add_node(
            "four-channel node after six-channel output".to_owned(),
            Box::new(FiniteFirProducer::new(4, Arc::clone(&last_calls))),
        )
        .unwrap();
    host.add_edge(GraphEdge::new(source, decoder)).unwrap();
    host.add_edge(GraphEdge::new(decoder, incompatible))
        .unwrap();
    host.build().unwrap();

    let error = host.drain(&mut [0.0; 4]).unwrap_err();
    assert!(
        error.contains("contiguous node widths"),
        "unexpected adjacent-width rejection: {error}"
    );
    assert_eq!(first_calls.begins.load(Ordering::Relaxed), 0);
    assert_eq!(first_calls.drains.load(Ordering::Relaxed), 0);
    assert_eq!(last_calls.begins.load(Ordering::Relaxed), 0);
    assert_eq!(last_calls.drains.load(Ordering::Relaxed), 0);
}

#[test]
#[ignore = "manual AUD140 pre-edit drain refusal and non-advancement capture"]
fn capture_aud140_pre_edit_refusal_leaves_finite_producer_unadvanced() {
    let input = finite_tail_input();
    let (mut host, calls) = make_order7_finite_tail_host();
    let mut process_output = vec![f32::NAN; 97 * ORDER7_OUTPUT_CHANNELS];
    assert_eq!(host.process(&input, &mut process_output).unwrap(), 97);

    let capacity_frames = host.drain_output_frames_max();
    assert_eq!(capacity_frames, 1);
    let mut drain_output = vec![f32::NAN; capacity_frames * ORDER7_OUTPUT_CHANNELS];
    let result = host.drain(&mut drain_output);
    let error = result.expect_err("pre-AUD140 source should refuse the width-changing chain");
    let begins = calls.begins.load(Ordering::Relaxed);
    let drains = calls.drains.load(Ordering::Relaxed);
    assert_eq!(begins, 0, "refused preflight must not begin producer drain");
    assert_eq!(
        drains, 0,
        "refused preflight must not consume producer tail"
    );

    let expected_producer = finite_fir_reference(&input, ORDER7_INPUT_CHANNELS);
    let expected_tail = &expected_producer[input.len()..];
    drop(host.remove_plugin(1).unwrap());
    host.build().unwrap();
    assert_eq!(host.output_channels(), ORDER7_INPUT_CHANNELS);
    let capacity_frames = host.drain_output_frames_max();
    assert_eq!(capacity_frames, 1);
    let mut recovered_tail = Vec::new();
    let mut complete = false;
    for _ in 0..FIR_TAIL_FRAMES + 1 {
        let mut chunk = vec![f32::NAN; capacity_frames * ORDER7_INPUT_CHANNELS];
        let result = host.drain(&mut chunk).unwrap();
        assert!(result.frames <= capacity_frames);
        recovered_tail.extend_from_slice(&chunk[..result.frames * ORDER7_INPUT_CHANNELS]);
        if result.complete {
            complete = true;
            break;
        }
    }
    assert!(
        complete,
        "preserved producer tail must complete after legal rewiring"
    );
    assert_eq!(recovered_tail.len(), expected_tail.len());
    assert_close(&recovered_tail, expected_tail, 1e-6);
    let begins_after_rewire = calls.begins.load(Ordering::Relaxed);
    let drains_after_rewire = calls.drains.load(Ordering::Relaxed);
    assert_eq!(begins_after_rewire, 1);
    assert_eq!(drains_after_rewire, FIR_TAIL_FRAMES);

    let directory = baseline_directory();
    fs::create_dir_all(&directory).unwrap();
    write_baseline_vector(
        &directory,
        "aud140-pre-edit-recovered-fir-tail",
        &recovered_tail,
    );
    fs::write(
        directory.join("aud140-pre-edit-drain-refusal.txt"),
        format!(
            "error={error}\nrefused_begin_drain_calls={begins}\nrefused_drain_calls={drains}\nproducer_begin_drain_calls_after_rewire={begins_after_rewire}\nproducer_drain_calls_after_rewire={drains_after_rewire}\nrecovered_tail_frames={}\n",
            recovered_tail.len() / ORDER7_INPUT_CHANNELS
        ),
    )
    .unwrap();
}

#[test]
fn width_changing_bypass_refusal_preserves_ordinary_audio_and_pending_tail() {
    let input = finite_tail_input();
    let filtered = finite_fir_reference(&input, ORDER7_INPUT_CHANNELS);
    let expected_process =
        process_ambisonics(&filtered[..input.len()], &ambisonics_config(7, "9.1.6"));
    let expected_tail =
        process_ambisonics(&filtered[input.len()..], &ambisonics_config(7, "9.1.6"));
    let (mut host, calls) = make_order7_finite_tail_host();
    let error = host.bypass_plugin(1).unwrap_err();
    assert!(error.contains("input channels") && error.contains("output channels"));

    let mut process_output = vec![f32::NAN; 97 * ORDER7_OUTPUT_CHANNELS];
    assert_eq!(host.process(&input, &mut process_output).unwrap(), 97);
    assert_close(&process_output, &expected_process, 1e-6);

    let mut actual_tail = Vec::new();
    let mut complete = false;
    for _ in 0..FIR_TAIL_FRAMES + 1 {
        let mut chunk = vec![f32::NAN; ORDER7_OUTPUT_CHANNELS];
        let result = host.drain(&mut chunk).unwrap();
        actual_tail.extend_from_slice(&chunk[..result.frames * ORDER7_OUTPUT_CHANNELS]);
        if result.complete {
            complete = true;
            break;
        }
    }
    assert!(complete);
    assert_eq!(actual_tail.len(), expected_tail.len());
    assert_close(&actual_tail, &expected_tail, 1e-6);
    assert_eq!(calls.begins.load(Ordering::Relaxed), 1);
    assert_eq!(calls.drains.load(Ordering::Relaxed), FIR_TAIL_FRAMES);
}

#[test]
#[ignore = "manual AUD140 pre-edit ordinary host audio capture"]
fn capture_aud140_pre_edit_ordinary_host_vectors() {
    let directory = baseline_directory();
    fs::create_dir_all(&directory).unwrap();
    for (name, samples) in ordinary_audio_cases() {
        write_baseline_vector(&directory, &name, &samples);
    }
}

#[test]
#[ignore = "manual AUD140 pre-edit ordinary host audio replay"]
fn replay_aud140_pre_edit_ordinary_host_vectors() {
    let directory = baseline_directory();
    for (name, samples) in ordinary_audio_cases() {
        assert_eq!(
            samples,
            read_baseline_vector(&directory, &name),
            "ordinary host processing changed for {name}"
        );
    }
}
