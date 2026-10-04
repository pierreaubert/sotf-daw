//! AUD137 independent finite-response, lifecycle, and host composition checks.

// Rust guideline compliant 2026-02-21
use math_audio_iir_fir::{Biquad, BiquadFilterType};
use serde_json::{Value, json};
use sotf_host::host::DawHost;
use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::{Plugin, PluginDrainResult, PluginInfo, ProcessContext, TailLength};
use sotf_plugin_ab_compare::{
    ABComparePlugin, ABComparePluginParams, GraphEdgeConfig, GraphNodeConfig, PathConfig,
    PluginInRack,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::num::NonZeroU64;

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: usize = 2;
const ACCEPTED_FRAMES: usize = 83;
const REPORTED_LATENCY_A: usize = 3;
const PATH_B_ALIGNMENT: usize = REPORTED_LATENCY_A;
const DRAIN_CALL_LIMIT: usize = 4096;

thread_local! {
    static TRACK_HEAP: Cell<bool> = const { Cell::new(false) };
    static HEAP_ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static HEAP_DEALLOCATIONS: Cell<usize> = const { Cell::new(0) };
    static COUNTED_BEGIN_DRAIN_CALLS: Cell<usize> = const { Cell::new(0) };
    static COUNTED_DRAIN_CALLS: Cell<usize> = const { Cell::new(0) };
    static FAILING_DRAIN_CALLS: Cell<usize> = const { Cell::new(0) };
}

struct Aud137CountingAllocator;

// SAFETY: Pointer and layout handling stays delegated to `System`; the
// instrumentation uses only constant-initialized, nonallocating TLS cells.
unsafe impl GlobalAlloc for Aud137CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let _ = TRACK_HEAP.try_with(|tracking| {
            if tracking.get() {
                let _ = HEAP_ALLOCATIONS.try_with(|count| count.set(count.get() + 1));
            }
        });
        // SAFETY: Pass the caller's valid layout unchanged to the system allocator.
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        let _ = TRACK_HEAP.try_with(|tracking| {
            if tracking.get() {
                let _ = HEAP_DEALLOCATIONS.try_with(|count| count.set(count.get() + 1));
            }
        });
        // SAFETY: Return the original pointer with its original layout.
        unsafe { System.dealloc(pointer, layout) }
    }
}

#[global_allocator]
static AUD137_ALLOCATOR: Aud137CountingAllocator = Aud137CountingAllocator;

fn heap_counts(f: impl FnOnce()) -> (usize, usize) {
    HEAP_ALLOCATIONS.set(0);
    HEAP_DEALLOCATIONS.set(0);
    TRACK_HEAP.set(true);
    f();
    TRACK_HEAP.set(false);
    (HEAP_ALLOCATIONS.get(), HEAP_DEALLOCATIONS.get())
}

#[derive(Clone, Copy)]
enum Route {
    Plugin,
    Rack,
    Graph,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FixtureRateMode {
    Identity,
    Double,
    Half,
}

struct FirFixture {
    channels: usize,
    taps: Vec<f32>,
    history: Vec<f32>,
    latency: usize,
    drain_capacity: usize,
    tail_remaining: usize,
    draining: bool,
    fail_on_drain: bool,
    count_drain_calls: bool,
    stall_first_drain: bool,
    has_stalled_drain: bool,
    non_identity_input_frames: Option<usize>,
    rate_mode: FixtureRateMode,
}

impl FirFixture {
    fn new(
        channels: usize,
        taps: Vec<f32>,
        latency: usize,
        drain_capacity: usize,
        fail_on_drain: bool,
        count_drain_calls: bool,
        stall_first_drain: bool,
    ) -> Result<Self, String> {
        if channels == 0 || taps.is_empty() || drain_capacity == 0 {
            return Err("AUD137 FIR fixture requires channels, taps, and drain capacity".into());
        }
        let history_samples = channels
            .checked_mul(taps.len())
            .ok_or_else(|| "AUD137 FIR fixture history size overflow".to_string())?;
        Ok(Self {
            channels,
            history: vec![0.0; history_samples],
            taps,
            latency,
            drain_capacity,
            tail_remaining: 0,
            draining: false,
            fail_on_drain,
            count_drain_calls,
            stall_first_drain,
            has_stalled_drain: false,
            non_identity_input_frames: None,
            rate_mode: FixtureRateMode::Identity,
        })
    }

    fn with_non_identity_input_frames(mut self, frames: usize) -> Self {
        self.non_identity_input_frames = Some(frames);
        self
    }

    fn with_rate_mode(mut self, rate_mode: FixtureRateMode) -> Self {
        self.rate_mode = rate_mode;
        self
    }

    fn expected_input_rate(&self) -> u32 {
        match self.rate_mode {
            FixtureRateMode::Identity | FixtureRateMode::Double => SAMPLE_RATE,
            FixtureRateMode::Half => SAMPLE_RATE * 2,
        }
    }

    fn filter_frame(&mut self, frame: &[f32], output: &mut [f32]) {
        for channel in 0..self.channels {
            output[channel] = self.filter_channel(channel, frame[channel]);
        }
    }

    fn filter_channel(&mut self, channel: usize, input: f32) -> f32 {
        let tap_count = self.taps.len();
        let start = channel * tap_count;
        self.history
            .copy_within(start..start + tap_count - 1, start + 1);
        self.history[start] = input;
        let mut accumulator = 0.0_f64;
        for (tap_index, &tap) in self.taps.iter().enumerate() {
            accumulator += f64::from(tap) * f64::from(self.history[start + tap_index]);
        }
        accumulator as f32
    }

    fn filter_silence_frame(&mut self, output: &mut [f32]) {
        for (channel, sample) in output.iter_mut().enumerate().take(self.channels) {
            *sample = self.filter_channel(channel, 0.0);
        }
    }
}

impl Plugin for FirFixture {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("AUD137 FIR fixture", "test", "SotF")
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

    fn set_parameter(&mut self, _id: ParameterId, _value: ParameterValue) -> Result<(), String> {
        Err("AUD137 FIR fixture has no parameters".into())
    }

    fn get_parameter(&self, _id: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn initialize(&mut self, sample_rate: f64) -> Result<(), String> {
        if sample_rate != f64::from(self.expected_input_rate()) {
            return Err(format!(
                "AUD137 FIR fixture expected {} Hz, got {sample_rate} Hz",
                self.expected_input_rate()
            ));
        }
        Ok(())
    }

    fn reset(&mut self) {
        self.history.fill(0.0);
        self.tail_remaining = 0;
        self.draining = false;
        self.has_stalled_drain = false;
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        let expected = context
            .num_frames
            .checked_mul(self.channels)
            .ok_or_else(|| "AUD137 FIR fixture block size overflow".to_string())?;
        // Staging is capacity, not exact size: live bounds are upper bounds,
        // so the unprobed-83 block stages 84 frames for 83 produced.
        if input.len() != expected || output.len() < expected {
            return Err("AUD137 FIR fixture received invalid process geometry".into());
        }
        if self.draining {
            return Err("AUD137 FIR fixture requires reset after drain begins".into());
        }
        if input.iter().any(|sample| !sample.is_finite()) {
            return Err("AUD137 FIR fixture input must be finite".into());
        }
        for frame in 0..context.num_frames {
            let start = frame * self.channels;
            self.filter_frame(
                &input[start..start + self.channels],
                &mut output[start..start + self.channels],
            );
        }
        Ok(context.num_frames)
    }

    fn latency_samples(&self) -> usize {
        self.latency
    }

    fn tail_length(&self) -> TailLength {
        TailLength::Finite(self.taps.len().saturating_sub(1) as u64)
    }

    fn output_frames_for_input(&self, input_frames: usize) -> usize {
        if self.non_identity_input_frames == Some(input_frames) {
            input_frames.saturating_add(1)
        } else {
            input_frames
        }
    }

    fn guarantees_identity_frame_geometry(&self) -> bool {
        self.non_identity_input_frames.is_none()
    }

    fn output_sample_rate(&self, input_rate: f64) -> f64 {
        match self.rate_mode {
            FixtureRateMode::Identity => input_rate,
            FixtureRateMode::Double => input_rate * 2.0,
            FixtureRateMode::Half => input_rate / 2.0,
        }
    }

    fn drain_output_frames_max(&self) -> usize {
        self.drain_capacity
    }

    fn begin_drain(&mut self, context: &ProcessContext) -> Result<(), String> {
        if context.num_frames != 0
            || context.sample_rate != f64::from(self.expected_input_rate())
        {
            return Err("AUD137 FIR fixture drain requires a zero-frame 48 kHz context".into());
        }
        if self.count_drain_calls {
            COUNTED_BEGIN_DRAIN_CALLS.with(|count| count.set(count.get() + 1));
        }
        self.draining = true;
        self.has_stalled_drain = false;
        self.tail_remaining = self.taps.len().saturating_sub(1);
        Ok(())
    }

    fn drain_call_bound(&self) -> Option<NonZeroU64> {
        let frames = self.taps.len().saturating_sub(1);
        let calls = (frames.div_ceil(self.drain_capacity) + usize::from(self.stall_first_drain))
            .max(1) as u64;
        NonZeroU64::new(calls)
    }

    fn drain(
        &mut self,
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<PluginDrainResult, String> {
        if self.fail_on_drain {
            FAILING_DRAIN_CALLS.with(|count| count.set(count.get() + 1));
            return Err("AUD137 fixture drain failure".into());
        }
        if !self.draining
            || context.num_frames != 0
            || context.sample_rate != f64::from(self.expected_input_rate())
        {
            return Err("AUD137 FIR fixture was not prepared for drain".into());
        }
        let required = self.drain_capacity * self.channels;
        if output.len() != required {
            return Err("AUD137 FIR fixture received the wrong drain capacity".into());
        }
        if self.stall_first_drain && !self.has_stalled_drain {
            self.has_stalled_drain = true;
            return Ok(PluginDrainResult {
                frames: 0,
                complete: false,
            });
        }
        let frames = self.tail_remaining.min(self.drain_capacity);
        for frame in 0..frames {
            let start = frame * self.channels;
            self.filter_silence_frame(&mut output[start..start + self.channels]);
        }
        self.tail_remaining -= frames;
        if self.count_drain_calls {
            COUNTED_DRAIN_CALLS.with(|count| count.set(count.get() + 1));
        }
        Ok(PluginDrainResult {
            frames,
            complete: self.tail_remaining == 0,
        })
    }
}

fn fir_factory(
    plugin_type: &str,
    parameters: &Value,
    channels: usize,
    _sample_rate: f64,
) -> Result<Box<dyn Plugin>, String> {
    let taps = parameters["taps"]
        .as_array()
        .ok_or_else(|| "AUD137 FIR fixture needs a taps array".to_string())?
        .iter()
        .map(|tap| {
            tap.as_f64()
                .map(|value| value as f32)
                .ok_or_else(|| "AUD137 FIR taps must be numeric".to_string())
        })
        .collect::<Result<Vec<_>, _>>()?;
    let latency = parameters["latency"].as_u64().unwrap_or(0) as usize;
    let drain_capacity = parameters["drain_capacity"].as_u64().unwrap_or(1) as usize;
    let fail_on_drain = plugin_type == "fail-on-drain";
    let count_drain_calls = matches!(
        plugin_type,
        "counted-fir" | "unprobed-counted-fir" | "rate-up-counted-fir" | "rate-down-counted-fir"
    );
    let stall_first_drain = plugin_type == "stall-once";
    let mut fixture = FirFixture::new(
        channels,
        taps,
        latency,
        drain_capacity,
        fail_on_drain,
        count_drain_calls,
        stall_first_drain,
    )?;
    if plugin_type == "unprobed-counted-fir" {
        fixture = fixture.with_non_identity_input_frames(83);
    }
    let rate_mode = match plugin_type {
        "rate-up-counted-fir" => FixtureRateMode::Double,
        "rate-down-counted-fir" => FixtureRateMode::Half,
        _ => FixtureRateMode::Identity,
    };
    fixture = fixture.with_rate_mode(rate_mode);
    Ok(Box::new(fixture))
}

fn path_config(
    route: Route,
    plugin_type: &str,
    taps: &[f32],
    latency: usize,
    drain_capacity: usize,
) -> PathConfig {
    let parameters = json!({
        "taps": taps,
        "latency": latency,
        "drain_capacity": drain_capacity,
    });
    match route {
        Route::Plugin => PathConfig::Plugin {
            plugin_type: plugin_type.to_owned(),
            parameters,
        },
        Route::Rack => PathConfig::Rack {
            plugins: vec![PluginInRack {
                plugin_type: plugin_type.to_owned(),
                parameters,
            }],
        },
        Route::Graph => PathConfig::Graph {
            nodes: vec![GraphNodeConfig {
                id: "fir".to_owned(),
                plugin_type: plugin_type.to_owned(),
                parameters,
            }],
            edges: Vec::<GraphEdgeConfig>::new(),
        },
    }
}

fn chained_path_config(route: Route, stages: &[(&[f32], usize)]) -> PathConfig {
    assert!(matches!(route, Route::Rack | Route::Graph));
    assert!(!stages.is_empty());
    let stage_parameters = stages
        .iter()
        .map(|(taps, latency)| {
            json!({
                "taps": taps,
                "latency": latency,
                "drain_capacity": 2,
            })
        })
        .collect::<Vec<_>>();
    match route {
        Route::Rack => PathConfig::Rack {
            plugins: stage_parameters
                .into_iter()
                .map(|parameters| PluginInRack {
                    plugin_type: "fir".to_owned(),
                    parameters,
                })
                .collect(),
        },
        Route::Graph => {
            let nodes = stage_parameters
                .into_iter()
                .enumerate()
                .map(|(index, parameters)| GraphNodeConfig {
                    id: format!("stage-{index}"),
                    plugin_type: "fir".to_owned(),
                    parameters,
                })
                .collect::<Vec<_>>();
            let edges = (1..nodes.len())
                .map(|index| GraphEdgeConfig {
                    from: format!("stage-{}", index - 1),
                    to: format!("stage-{index}"),
                    channel_map: None,
                    destination_offset: 0,
                })
                .collect();
            PathConfig::Graph { nodes, edges }
        }
        Route::Plugin => unreachable!("a plugin path cannot contain a serial chain"),
    }
}

fn make_fir_plugin(route: Route, auto_gain: bool) -> ABComparePlugin {
    let path_a = path_config(
        route,
        "fir",
        &[0.0, 0.0, 0.0, 1.0, 0.25],
        REPORTED_LATENCY_A,
        2,
    );
    let path_b = path_config(route, "fir", &[0.75, -0.2, 0.1], 0, 1);
    let mut params = ABComparePluginParams {
        path_a,
        path_b,
        mix: 0.0,
        auto_gain_enabled: auto_gain,
        ..ABComparePluginParams::default()
    };
    if auto_gain {
        params.mix = 1.0;
    }
    let mut plugin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, fir_factory)
            .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    plugin
}

fn make_fir_control_plugin(
    route: Route,
    mix: f32,
    difference_mode: bool,
    phase_invert_b: bool,
    bypass: bool,
) -> ABComparePlugin {
    let params = ABComparePluginParams {
        path_a: path_config(
            route,
            "fir",
            &[0.0, 0.0, 0.0, 1.0, 0.25],
            REPORTED_LATENCY_A,
            2,
        ),
        path_b: path_config(route, "fir", &[0.75, -0.2, 0.1], 0, 1),
        mix,
        difference_mode,
        phase_invert_b,
        bypass,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut plugin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, fir_factory)
            .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    plugin
}

fn dense_input(frames: usize) -> Vec<f32> {
    (0..frames * CHANNELS)
        .map(|index| {
            let frame = index / CHANNELS;
            let channel = index % CHANNELS;
            let value = ((frame * (7 + 4 * channel) + 3 * channel) % 23) as f32 - 11.0;
            value / 16.0
        })
        .collect()
}

fn render_chunks(plugin: &mut ABComparePlugin, input: &[f32], chunks: &[usize]) -> Vec<f32> {
    assert_eq!(chunks.iter().sum::<usize>() * CHANNELS, input.len());
    let mut output = vec![0.0; input.len()];
    let mut frame_offset = 0;
    for &frames in chunks {
        let start = frame_offset * CHANNELS;
        let end = start + frames * CHANNELS;
        let context = ProcessContext::new(SAMPLE_RATE, frames);
        plugin
            .process(&input[start..end], &mut output[start..end], &context)
            .unwrap();
        frame_offset += frames;
    }
    output
}

fn render_chunks_captured(
    plugin: &mut ABComparePlugin,
    input: &[f32],
    chunks: &[usize],
) -> (Vec<f32>, Vec<usize>) {
    assert_eq!(chunks.iter().sum::<usize>() * CHANNELS, input.len());
    let mut output = Vec::new();
    let mut returns = Vec::with_capacity(chunks.len());
    let mut frame_offset = 0;
    for &frames in chunks {
        let start = frame_offset * CHANNELS;
        let end = start + frames * CHANNELS;
        let mut block = vec![f32::NAN; frames * CHANNELS];
        let context = ProcessContext::new(SAMPLE_RATE, frames);
        let produced = plugin
            .process(&input[start..end], &mut block, &context)
            .unwrap();
        assert!(
            produced <= frames,
            "plugin emitted {produced} frames for a {frames}-frame block",
        );
        returns.push(produced);
        output.extend_from_slice(&block[..produced * CHANNELS]);
        frame_offset += frames;
    }
    (output, returns)
}

fn drain_all(plugin: &mut ABComparePlugin) -> (Vec<f32>, Vec<usize>) {
    let context = ProcessContext::new(SAMPLE_RATE, 0);
    plugin.begin_drain(&context).unwrap();
    let capacity_frames = plugin.drain_output_frames_max();
    let mut block = vec![f32::NAN; capacity_frames * CHANNELS];
    let mut output = Vec::new();
    let mut partition = Vec::new();
    for _ in 0..DRAIN_CALL_LIMIT {
        block.fill(f32::NAN);
        let result = plugin.drain(&mut block, &context).unwrap();
        assert!(result.frames <= capacity_frames);
        partition.push(result.frames);
        output.extend_from_slice(&block[..result.frames * CHANNELS]);
        if result.complete {
            return (output, partition);
        }
    }
    panic!("AUD137 drain exceeded the test's bounded call allowance");
}

fn convolve_f64(input: &[f32], taps: &[f32]) -> Vec<f32> {
    assert!(input.len().is_multiple_of(CHANNELS));
    let input_frames = input.len() / CHANNELS;
    let output_frames = input_frames + taps.len() - 1;
    let mut output = vec![0.0; output_frames * CHANNELS];
    for output_frame in 0..output_frames {
        for channel in 0..CHANNELS {
            let mut accumulator = 0.0_f64;
            for (tap_index, &tap) in taps.iter().enumerate() {
                if output_frame >= tap_index {
                    let input_frame = output_frame - tap_index;
                    if input_frame < input_frames {
                        accumulator +=
                            f64::from(input[input_frame * CHANNELS + channel]) * f64::from(tap);
                    }
                }
            }
            output[output_frame * CHANNELS + channel] = accumulator as f32;
        }
    }
    output
}

fn convolve_taps(first: &[f32], second: &[f32]) -> Vec<f32> {
    let mut combined = vec![0.0_f64; first.len() + second.len() - 1];
    for (first_index, &first_tap) in first.iter().enumerate() {
        for (second_index, &second_tap) in second.iter().enumerate() {
            combined[first_index + second_index] += f64::from(first_tap) * f64::from(second_tap);
        }
    }
    combined.into_iter().map(|tap| tap as f32).collect()
}

#[derive(Clone, Copy)]
struct ReferenceOptions {
    path_b_alignment: usize,
    mix: f32,
    difference_mode: bool,
    phase_invert_b: bool,
    bypass: bool,
}

fn direct_reference(
    input: &[f32],
    taps_a: &[f32],
    taps_b: &[f32],
    options: ReferenceOptions,
) -> Vec<f32> {
    let input_frames = input.len() / CHANNELS;
    let output_frames = input_frames + options.path_b_alignment + taps_b.len() - 1;
    let response_a = convolve_f64(input, taps_a);
    let response_b = convolve_f64(input, taps_b);
    let mut expected = vec![0.0; output_frames * CHANNELS];
    let mix_01 = (options.mix + 1.0) / 2.0;
    for frame in 0..output_frames {
        for channel in 0..CHANNELS {
            let a = response_a
                .get(frame * CHANNELS + channel)
                .copied()
                .unwrap_or(0.0);
            let b_frame = frame.checked_sub(options.path_b_alignment);
            let mut b = b_frame
                .and_then(|index| response_b.get(index * CHANNELS + channel))
                .copied()
                .unwrap_or(0.0);
            if options.phase_invert_b {
                b = -b;
            }
            let wet = if options.difference_mode {
                f64::from(a) - f64::from(b)
            } else {
                f64::from(a) * f64::from(1.0 - mix_01) + f64::from(b) * f64::from(mix_01)
            };
            let dry = frame
                .checked_sub(REPORTED_LATENCY_A)
                .and_then(|source_frame| input.get(source_frame * CHANNELS + channel))
                .copied()
                .unwrap_or(0.0);
            expected[frame * CHANNELS + channel] = if options.bypass { dry } else { wet as f32 };
        }
    }
    expected
}

fn reference_default(input: &[f32]) -> Vec<f32> {
    direct_reference(
        input,
        &[0.0, 0.0, 0.0, 1.0, 0.25],
        &[0.75, -0.2, 0.1],
        ReferenceOptions {
            path_b_alignment: PATH_B_ALIGNMENT,
            mix: 0.0,
            difference_mode: false,
            phase_invert_b: false,
            bypass: false,
        },
    )
}

fn assert_vectors_close(actual: &[f32], expected: &[f32], name: &str) {
    assert_eq!(actual.len(), expected.len(), "{name} full-stream length");
    for (index, (&actual, &expected)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (actual - expected).abs() <= 1.0e-6,
            "{name} sample {index}: expected {expected}, got {actual}"
        );
    }
}

#[test]
fn plugin_rack_and_graph_paths_match_an_independent_f64_fir_oracle() {
    let input = dense_input(ACCEPTED_FRAMES);
    let expected = reference_default(&input);
    assert_eq!(expected.len() / CHANNELS, ACCEPTED_FRAMES + 5);
    assert!(
        expected[ACCEPTED_FRAMES * CHANNELS..]
            .iter()
            .any(|sample| sample.abs() > 1.0e-3)
    );

    for route in [Route::Plugin, Route::Rack, Route::Graph] {
        let route_name = match route {
            Route::Plugin => "Plugin",
            Route::Rack => "Rack",
            Route::Graph => "Graph",
        };
        let mut plugin = make_fir_plugin(route, false);
        assert_eq!(plugin.latency_samples(), REPORTED_LATENCY_A);
        // Structural tail, exact in every state: path A contributes its
        // FIR ring-out (5 taps - 1 = 4) with no alignment delay, path B
        // its ring-out (3 taps - 1 = 2) after the 3-frame alignment
        // delay (4 vs 5), and the dry lane its 3-frame latency delay;
        // the mixer maximum is 5. Both FIRs arm taps-1 drain frames
        // regardless of history and the delays flush structurally, so
        // the fresh-state query below is exact, not merely a bound —
        // the fresh-drain proof after this loop confirms the emission.
        assert_eq!(plugin.tail_length(), TailLength::Finite(5));
        assert_eq!(plugin.drain_output_frames_max(), 3);
        let process_output = render_chunks(&mut plugin, &input, &[1, 7, 19, 3, 53]);
        let (drain_output, drain_partition) = drain_all(&mut plugin);
        assert!(drain_partition.iter().all(|&frames| frames <= 3));
        let mut actual = process_output;
        actual.extend_from_slice(&drain_output);
        assert_vectors_close(&actual, &expected, route_name);
    }

    // Fresh-state exactness proof for the Finite(5) above: an unprocessed
    // plugin drains exactly 5 frames of exact zero (either sign; structural
    // arms fire from zero histories), so the pre-process query is the true
    // count.
    for route in [Route::Plugin, Route::Rack, Route::Graph] {
        let route_name = match route {
            Route::Plugin => "fresh Plugin",
            Route::Rack => "fresh Rack",
            Route::Graph => "fresh Graph",
        };
        let mut plugin = make_fir_plugin(route, false);
        assert_eq!(plugin.tail_length(), TailLength::Finite(5));
        let (drain_output, _) = drain_all(&mut plugin);
        assert_eq!(
            drain_output.len() / CHANNELS,
            5,
            "{route_name} fresh drain emits exactly the queried tail"
        );
        assert!(
            drain_output.iter().all(|&sample| sample == 0.0),
            "{route_name} fresh drain content is exactly zero"
        );
        assert_eq!(plugin.tail_length(), TailLength::Finite(0));
    }
}

#[test]
fn drain_mixer_controls_match_independent_complete_vectors() {
    let input = dense_input(ACCEPTED_FRAMES);
    let controls = [
        ("pure A", -1.0, false, false, false),
        ("pure B", 1.0, false, false, false),
        ("50/50", 0.0, false, false, false),
        ("difference", 0.0, true, false, false),
        ("phase inverted B", 1.0, false, true, false),
        ("bypass", 0.0, false, false, true),
    ];
    for (name, mix, difference, phase_invert_b, bypass) in controls {
        let expected = direct_reference(
            &input,
            &[0.0, 0.0, 0.0, 1.0, 0.25],
            &[0.75, -0.2, 0.1],
            ReferenceOptions {
                path_b_alignment: PATH_B_ALIGNMENT,
                mix,
                difference_mode: difference,
                phase_invert_b,
                bypass,
            },
        );
        let mut plugin =
            make_fir_control_plugin(Route::Plugin, mix, difference, phase_invert_b, bypass);
        let process_output = render_chunks(&mut plugin, &input, &[1, 7, 19, 3, 53]);
        let (drain_output, _) = drain_all(&mut plugin);
        let mut actual = process_output;
        actual.extend_from_slice(&drain_output);
        assert_vectors_close(&actual, &expected, name);
    }
}

#[test]
fn two_stage_rack_and_graph_routes_preserve_nested_causality() {
    let input = dense_input(ACCEPTED_FRAMES);
    let taps_a_stage_1 = [0.0, 0.0, 1.0, 0.25];
    let taps_a_stage_2 = [1.0, 0.5];
    let taps_b_stage_1 = [0.8, -0.1, 0.05];
    let taps_b_stage_2 = [1.0, 0.25];
    let taps_a = convolve_taps(&taps_a_stage_1, &taps_a_stage_2);
    let taps_b = convolve_taps(&taps_b_stage_1, &taps_b_stage_2);
    let expected = direct_reference(
        &input,
        &taps_a,
        &taps_b,
        ReferenceOptions {
            path_b_alignment: 2,
            mix: 0.0,
            difference_mode: false,
            phase_invert_b: false,
            bypass: false,
        },
    );

    for route in [Route::Rack, Route::Graph] {
        let path_a = chained_path_config(route, &[(&taps_a_stage_1, 2), (&taps_a_stage_2, 0)]);
        let path_b = chained_path_config(route, &[(&taps_b_stage_1, 0), (&taps_b_stage_2, 0)]);
        let params = ABComparePluginParams {
            path_a,
            path_b,
            mix: 0.0,
            auto_gain_enabled: false,
            ..ABComparePluginParams::default()
        };
        let mut plugin =
            ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, fir_factory)
                .unwrap();
        plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
        assert_eq!(plugin.latency_samples(), 2);
        let process_output = render_chunks(&mut plugin, &input, &[1, 17, 2, 31, 32]);
        let (drain_output, _) = drain_all(&mut plugin);
        let mut actual = process_output;
        actual.extend_from_slice(&drain_output);
        assert_vectors_close(
            &actual,
            &expected,
            match route {
                Route::Rack => "two-stage Rack",
                Route::Graph => "two-stage Graph",
                Route::Plugin => unreachable!(),
            },
        );
    }
}

#[test]
fn drain_preflight_is_transactional_and_completion_requires_reset() {
    let input = dense_input(ACCEPTED_FRAMES);
    let expected = reference_default(&input);
    let mut plugin = make_fir_plugin(Route::Plugin, false);
    let process_output = render_chunks(&mut plugin, &input, &[83]);
    let wrong_rate = ProcessContext::new(SAMPLE_RATE - 1, 0);
    assert!(plugin.begin_drain(&wrong_rate).is_err());
    let context = ProcessContext::new(SAMPLE_RATE, 0);
    plugin.begin_drain(&context).unwrap();

    let capacity_frames = plugin.drain_output_frames_max();
    assert!(capacity_frames > 0);
    assert!(plugin.drain(&mut [], &context).is_err());
    let mut too_small = vec![f32::NAN; (capacity_frames - 1) * CHANNELS];
    assert!(plugin.drain(&mut too_small, &context).is_err());
    let mut misaligned = vec![f32::NAN; capacity_frames * CHANNELS + 1];
    assert!(plugin.drain(&mut misaligned, &context).is_err());
    assert!(
        plugin
            .set_parameter(ParameterId::from("mix"), ParameterValue::Float(0.25))
            .is_err()
    );
    let mut rejected_process = vec![f32::NAN; CHANNELS];
    assert!(
        plugin
            .process(
                &[0.0; CHANNELS],
                &mut rejected_process,
                &ProcessContext::new(SAMPLE_RATE, 1),
            )
            .is_err()
    );
    assert!(rejected_process.iter().all(|sample| sample.is_nan()));
    assert_eq!(plugin.process(&[], &mut [], &context).unwrap(), 0);

    let (drain_output, _) = drain_all_after_begin(&mut plugin, &context);
    let mut actual = process_output.clone();
    actual.extend_from_slice(&drain_output);
    assert_vectors_close(&actual, &expected, "transactional preflight");
    assert!(plugin.drain(&mut [], &context).unwrap().complete);
    assert!(
        plugin
            .set_parameter(ParameterId::from("mix"), ParameterValue::Float(0.25))
            .is_err()
    );
    assert!(
        plugin
            .process(
                &[0.0; CHANNELS],
                &mut [f32::NAN; CHANNELS],
                &ProcessContext::new(SAMPLE_RATE, 1),
            )
            .is_err()
    );

    plugin.reset();
    let after_reset = render_chunks(&mut plugin, &input, &[83]);
    assert_eq!(after_reset, process_output);
    let (reset_tail, _) = drain_all(&mut plugin);
    assert_eq!(reset_tail, drain_output);

    let mut fresh = make_fir_plugin(Route::Plugin, false);
    let fresh_process = render_chunks(&mut fresh, &input, &[83]);
    let (fresh_tail, _) = drain_all(&mut fresh);
    assert_eq!(after_reset, fresh_process);
    assert_eq!(reset_tail, fresh_tail);
}

#[test]
fn zero_output_child_progress_can_resume_and_preserve_the_tail() {
    let input = dense_input(ACCEPTED_FRAMES);
    let taps_a = [1.0, 0.5];
    let taps_b = [0.75, -0.25];
    let expected = direct_reference(
        &input,
        &taps_a,
        &taps_b,
        ReferenceOptions {
            path_b_alignment: 0,
            mix: -1.0,
            difference_mode: false,
            phase_invert_b: false,
            bypass: false,
        },
    );
    let params = ABComparePluginParams {
        path_a: path_config(Route::Plugin, "stall-once", &taps_a, 0, 1),
        path_b: path_config(Route::Plugin, "fir", &taps_b, 0, 1),
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut plugin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, fir_factory)
            .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let process_output = render_chunks(&mut plugin, &input, &[83]);
    let context = ProcessContext::new(SAMPLE_RATE, 0);
    plugin.begin_drain(&context).unwrap();

    let capacity_frames = plugin.drain_output_frames_max();
    let mut block = vec![f32::NAN; capacity_frames * CHANNELS];
    let no_progress = plugin.drain(&mut block, &context).unwrap();
    assert_eq!(no_progress.frames, 0);
    assert!(!no_progress.complete);
    assert!(block.iter().all(|sample| sample.is_nan()));

    let mut drain_output = Vec::new();
    for _ in 0..DRAIN_CALL_LIMIT {
        block.fill(f32::NAN);
        let result = plugin.drain(&mut block, &context).unwrap();
        drain_output.extend_from_slice(&block[..result.frames * CHANNELS]);
        if result.complete {
            let mut actual = process_output;
            actual.extend_from_slice(&drain_output);
            assert_vectors_close(&actual, &expected, "zero-output progress recovery");
            return;
        }
    }
    panic!("AUD137 zero-output child did not resume within the drain bound");
}

/// Fresh-`Biquad` HP->LP cascade over a known wet sequence, mirroring the
/// production mixer's per-sample operations exactly.
fn mask_cascade_reference(wet: &[f32], low_hz: f64, high_hz: f64) -> Vec<f32> {
    assert!(wet.len().is_multiple_of(CHANNELS));
    let q = 1.0 / std::f64::consts::SQRT_2;
    let rate = f64::from(SAMPLE_RATE);
    let mut highpass: Vec<Biquad> = (0..CHANNELS)
        .map(|_| Biquad::new(BiquadFilterType::Highpass, low_hz, rate, q, 0.0))
        .collect();
    let mut lowpass: Vec<Biquad> = (0..CHANNELS)
        .map(|_| Biquad::new(BiquadFilterType::Lowpass, high_hz, rate, q, 0.0))
        .collect();
    let mut output = Vec::with_capacity(wet.len());
    for frame in wet.as_chunks::<CHANNELS>().0 {
        for (channel, &sample) in frame.iter().enumerate() {
            let hp_out = highpass[channel].process(f64::from(sample));
            output.push(lowpass[channel].process(hp_out) as f32);
        }
    }
    output
}

#[test]
fn active_band_mask_drains_proven_residual_and_completes() {
    // R2 supersession (root disposition 2026-10-01: mask refusal is not
    // completion): latency-bearing FIR paths plus an active mask drain the
    // child tails, the (empty here) latency rings, and the proven residual
    // flush. The whole stream matches the masked f64 FIR oracle at 1e-6 and
    // leaves a remainder below wet peak x 2^-24.
    COUNTED_DRAIN_CALLS.set(0);
    let taps_a = [1.0, 0.5];
    let taps_b = [1.0, 0.25];
    let params = ABComparePluginParams {
        path_a: path_config(Route::Plugin, "counted-fir", &taps_a, 0, 1),
        path_b: path_config(Route::Plugin, "counted-fir", &taps_b, 0, 1),
        mix: 0.0,
        band_mask_low_hz: 500.0,
        band_mask_high_hz: 8_000.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut plugin = ABComparePlugin::from_params_with_factory(
        CHANNELS,
        SAMPLE_RATE,
        params.clone(),
        fir_factory,
    )
    .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let input = dense_input(ACCEPTED_FRAMES);
    let nomask = direct_reference(
        &input,
        &taps_a,
        &taps_b,
        ReferenceOptions {
            path_b_alignment: 0,
            mix: 0.0,
            difference_mode: false,
            phase_invert_b: false,
            bypass: false,
        },
    );
    assert_eq!(nomask.len() / CHANNELS, ACCEPTED_FRAMES + 1);

    let process_output = render_chunks(&mut plugin, &input, &[1, 7, 19, 3, 53]);
    let (drain_output, _) = drain_all(&mut plugin);
    assert!(
        COUNTED_DRAIN_CALLS.get() > 0,
        "both FIR children must drain independently"
    );
    let mut actual = process_output;
    actual.extend_from_slice(&drain_output);
    let flush_frames = actual.len() / CHANNELS - nomask.len() / CHANNELS;
    assert!(
        flush_frames >= 64,
        "the residual flush must engage nontrivially, got {flush_frames}"
    );
    let mut wet = nomask.clone();
    wet.extend(std::iter::repeat_n(0.0, flush_frames * CHANNELS));
    let expected = mask_cascade_reference(&wet, 500.0, 8_000.0);
    assert_vectors_close(&actual, &expected, "masked FIR drain");
    let wet_peak = nomask
        .iter()
        .fold(0.0_f64, |max, sample| max.max(f64::from(sample.abs())));
    assert!(wet_peak > 0.0);
    let threshold = wet_peak / 16_777_216.0;
    let mut extended = wet.clone();
    extended.extend(std::iter::repeat_n(0.0, 20_000 * CHANNELS));
    let sounded = mask_cascade_reference(&extended, 500.0, 8_000.0);
    let tail_max = sounded[wet.len()..]
        .iter()
        .fold(0.0_f64, |max, sample| max.max(f64::from(sample.abs())));
    assert!(
        tail_max < threshold,
        "remainder {tail_max} must stay below {threshold}"
    );

    // Immediate drain with no program: zero excitation means zero flush; only
    // the one zero-valued FIR tail frame flows (masked zeros stay zeros).
    let mut fresh =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, fir_factory)
            .unwrap();
    fresh.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let (tail, _) = drain_all(&mut fresh);
    assert_eq!(tail.len(), CHANNELS, "one zero tail frame, no flush");
    assert!(tail.iter().all(|sample| *sample == 0.0));
}

#[test]
fn nonlinear_graph_joins_and_drains_exactly() {
    // The R20 disposition supersedes the nonlinear-graph drain refusal:
    // heterogeneous branches (different FIR taps) feed the transparent
    // output join; the merge sums them losslessly in process and at EOF.
    // The oracle feeds two raw fixtures and sums — bitwise.
    COUNTED_BEGIN_DRAIN_CALLS.set(0);
    COUNTED_DRAIN_CALLS.set(0);
    let taps_a = [1.0, 0.5];
    let taps_b = [0.75, -0.25];
    let params = ABComparePluginParams {
        path_a: PathConfig::Graph {
            nodes: vec![
                GraphNodeConfig {
                    id: "branch-a".to_owned(),
                    plugin_type: "counted-fir".to_owned(),
                    parameters: json!({ "taps": [1.0, 0.5], "drain_capacity": 1 }),
                },
                GraphNodeConfig {
                    id: "branch-b".to_owned(),
                    plugin_type: "counted-fir".to_owned(),
                    parameters: json!({ "taps": [0.75, -0.25], "drain_capacity": 1 }),
                },
            ],
            edges: Vec::new(),
        },
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut plugin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, fir_factory)
            .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    assert_eq!(plugin.latency_samples(), 0);

    let chunks = [83_usize, 64, 7, 129, 1];
    assert_eq!(chunks.iter().sum::<usize>(), 284);
    let input = dense_input(284);
    let (mut whole, returns) = render_chunks_captured(&mut plugin, &input, &chunks);
    assert_eq!(
        returns, chunks,
        "identity branches emit full counts through the join",
    );
    let (tail, partition) = drain_all(&mut plugin);
    whole.extend_from_slice(&tail);
    assert_eq!(
        returns.iter().sum::<usize>() + partition.iter().sum::<usize>(),
        whole.len() / CHANNELS,
        "count conservation",
    );

    // Raw per-branch references, summed elementwise in config edge order.
    let mut expected = vec![0.0; whole.len()];
    for taps in [taps_a.as_slice(), taps_b.as_slice()] {
        let mut raw = FirFixture::new(CHANNELS, taps.to_vec(), 0, 1, false, false, false).unwrap();
        raw.initialize(f64::from(SAMPLE_RATE)).unwrap();
        let mut reference = vec![0.0; input.len()];
        let produced = raw
            .process(
                &input,
                &mut reference,
                &ProcessContext::new(SAMPLE_RATE, 284),
            )
            .unwrap();
        assert_eq!(produced, 284);
        raw.begin_drain(&ProcessContext::new(SAMPLE_RATE, 0))
            .unwrap();
        let mut slice = vec![0.0; CHANNELS];
        let mut branch_complete = false;
        for _ in 0..4 {
            let result = raw
                .drain(&mut slice, &ProcessContext::new(SAMPLE_RATE, 0))
                .unwrap();
            reference.extend_from_slice(&slice[..result.frames * CHANNELS]);
            if result.complete {
                branch_complete = true;
                break;
            }
        }
        assert!(branch_complete, "raw branch must drain within its bound");
        assert_eq!(reference.len(), expected.len());
        for (slot, sample) in expected.iter_mut().zip(reference.iter()) {
            *slot += sample;
        }
    }
    assert_eq!(
        whole, expected,
        "joined hetero content must match the raw sum bitwise",
    );
    println!("nonlinear join: drain partition {partition:?}");
    assert!(
        COUNTED_DRAIN_CALLS.get() > 0,
        "both branches must actually drain",
    );
    let context = ProcessContext::new(SAMPLE_RATE, 0);
    let terminal = plugin.drain(&mut [], &context).unwrap();
    assert!(
        terminal.complete && terminal.frames == 0,
        "post-complete drain must stay complete with zero frames",
    );
}

#[test]
fn unprobed_geometry_composes_with_truthful_counts() {
    // The R20 disposition supersedes the unprobed-geometry refusal: a child
    // whose declaration changes at input length 83 (where no fixed probe
    // looks) still pairs frames by stream position, because unpadded
    // process returns and actual drain returns report truth, not bounds.
    // The 83-frame block leads, so the anomaly is the proof, not the gap.
    COUNTED_BEGIN_DRAIN_CALLS.set(0);
    COUNTED_DRAIN_CALLS.set(0);
    let params = ABComparePluginParams {
        path_a: path_config(Route::Plugin, "unprobed-counted-fir", &[1.0, 0.5], 0, 2),
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut plugin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, fir_factory)
            .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    assert_eq!(plugin.latency_samples(), 0);

    let chunks = [ACCEPTED_FRAMES, 64, 7, 129, 1];
    assert_eq!(chunks.iter().sum::<usize>(), 284);
    let input = dense_input(284);
    let (mut whole, returns) = render_chunks_captured(&mut plugin, &input, &chunks);
    assert_eq!(
        returns, chunks,
        "every per-call count must be truthful, 83 included",
    );
    let (tail, partition) = drain_all(&mut plugin);
    whole.extend_from_slice(&tail);
    assert_eq!(
        returns.iter().sum::<usize>() + partition.iter().sum::<usize>(),
        whole.len() / CHANNELS,
        "count conservation",
    );
    let expected = convolve_f64(&input, &[1.0, 0.5]);
    assert_vectors_close(&whole, &expected, "unprobed geometry");
    println!("unprobed geometry: drain partition {partition:?}");
    assert!(
        COUNTED_DRAIN_CALLS.get() > 0,
        "the child must actually drain",
    );
    let context = ProcessContext::new(SAMPLE_RATE, 0);
    let terminal = plugin.drain(&mut [], &context).unwrap();
    assert!(
        terminal.complete && terminal.frames == 0,
        "post-complete drain must stay complete with zero frames",
    );
}

#[test]
fn compensating_node_rates_compose_silently() {
    // The R20 disposition supersedes the compensating-rates refusal: silent
    // rate-changing stages that fold back to the outer clock (48→96→48)
    // compose with truthful per-call counts and bit-exact passthrough
    // content, draining coherently to EOF. Nothing is hidden: every count
    // is asserted against its block.
    COUNTED_BEGIN_DRAIN_CALLS.set(0);
    COUNTED_DRAIN_CALLS.set(0);
    let stage_parameters = json!({
        "taps": [1.0],
        "latency": 0,
        "drain_capacity": 2,
    });
    let params = ABComparePluginParams {
        path_a: PathConfig::Rack {
            plugins: vec![
                PluginInRack {
                    plugin_type: "rate-up-counted-fir".to_owned(),
                    parameters: stage_parameters.clone(),
                },
                PluginInRack {
                    plugin_type: "rate-down-counted-fir".to_owned(),
                    parameters: stage_parameters,
                },
            ],
        },
        path_b: PathConfig::None,
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut plugin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, fir_factory)
            .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    assert_eq!(plugin.latency_samples(), 0);

    let chunks = [83_usize, 64, 7, 129, 1];
    assert_eq!(chunks.iter().sum::<usize>(), 284);
    let input = dense_input(284);
    let (mut whole, returns) = render_chunks_captured(&mut plugin, &input, &chunks);
    assert_eq!(
        returns, chunks,
        "silent compensating counts must match every block",
    );
    let (tail, partition) = drain_all(&mut plugin);
    whole.extend_from_slice(&tail);
    assert!(
        tail.is_empty(),
        "passthrough stages drain empty, got {} frames",
        tail.len() / CHANNELS,
    );
    assert_eq!(
        whole, input,
        "silent compensating content must pass through bit-exactly",
    );
    println!("compensating rates: drain partition {partition:?}");
    assert!(
        COUNTED_DRAIN_CALLS.get() > 0,
        "the children must actually drain",
    );
    let context = ProcessContext::new(SAMPLE_RATE, 0);
    let terminal = plugin.drain(&mut [], &context).unwrap();
    assert!(
        terminal.complete && terminal.frames == 0,
        "post-complete drain must stay complete with zero frames",
    );
}

#[test]
fn empty_no_tail_stream_completes_with_zero_frames() {
    let mut plugin = make_no_tail_child_plugin();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    assert_eq!(plugin.drain_output_frames_max(), 17);

    let context = ProcessContext::new(SAMPLE_RATE, 0);
    plugin.begin_drain(&context).unwrap();
    let mut output = vec![f32::NAN; 17 * CHANNELS];
    let result = plugin.drain(&mut output, &context).unwrap();

    assert_eq!(result.frames, 0, "an empty stream has no padded output");
    assert!(result.complete);
    assert!(output.iter().all(|sample| sample.is_nan()));
    let repeated = plugin.drain(&mut output, &context).unwrap();
    assert_eq!(repeated.frames, 0);
    assert!(repeated.complete);

    plugin.reset();
    let mut process_output = [f32::NAN; CHANNELS];
    assert_eq!(
        plugin
            .process(
                &[0.0; CHANNELS],
                &mut process_output,
                &ProcessContext::new(SAMPLE_RATE, 1),
            )
            .unwrap(),
        1
    );
    plugin.begin_drain(&context).unwrap();
    output.fill(f32::NAN);
    let after_input = plugin.drain(&mut output, &context).unwrap();
    assert_eq!(after_input.frames, 0);
    assert!(after_input.complete);
    assert!(output.iter().all(|sample| sample.is_nan()));
}

fn make_no_tail_child_plugin() -> ABComparePlugin {
    let params = ABComparePluginParams {
        path_a: path_config(Route::Plugin, "fir", &[1.0], 0, 17),
        path_b: PathConfig::None,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, fir_factory).unwrap()
}

#[test]
fn short_dry_only_tail_emits_exact_remaining_frames() {
    const DRY_DELAY_FRAMES: usize = 3;
    const CHILD_CAPACITY_FRAMES: usize = 16;
    const SETTLE_FRAMES: usize = 1_024;
    let make_plugin = || {
        let params = ABComparePluginParams {
            path_a: path_config(
                Route::Plugin,
                "fir",
                &[1.0],
                DRY_DELAY_FRAMES,
                CHILD_CAPACITY_FRAMES,
            ),
            path_b: path_config(
                Route::Plugin,
                "fir",
                &[1.0],
                DRY_DELAY_FRAMES,
                CHILD_CAPACITY_FRAMES,
            ),
            bypass: true,
            auto_gain_enabled: false,
            ..ABComparePluginParams::default()
        };
        let mut plugin =
            ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, fir_factory)
                .unwrap();
        plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
        plugin
    };
    let mut plugin = make_plugin();
    let mut continued = make_plugin();

    let mut input = vec![0.0; (SETTLE_FRAMES + 1) * CHANNELS];
    let final_frame = SETTLE_FRAMES * CHANNELS;
    input[final_frame] = 0.75;
    input[final_frame + 1] = -0.25;
    let mut process_output = vec![f32::NAN; input.len()];
    plugin
        .process(
            &input,
            &mut process_output,
            &ProcessContext::new(SAMPLE_RATE, SETTLE_FRAMES + 1),
        )
        .unwrap();
    continued
        .process(
            &input,
            &mut vec![f32::NAN; input.len()],
            &ProcessContext::new(SAMPLE_RATE, SETTLE_FRAMES + 1),
        )
        .unwrap();

    let context = ProcessContext::new(SAMPLE_RATE, 0);
    let zero_input = vec![0.0; DRY_DELAY_FRAMES * CHANNELS];
    let mut expected_tail = vec![f32::NAN; zero_input.len()];
    continued
        .process(
            &zero_input,
            &mut expected_tail,
            &ProcessContext::new(SAMPLE_RATE, DRY_DELAY_FRAMES),
        )
        .unwrap();
    assert!(expected_tail.iter().any(|sample| sample.abs() > 0.1));
    plugin.begin_drain(&context).unwrap();
    assert_eq!(plugin.drain_output_frames_max(), CHILD_CAPACITY_FRAMES);
    let mut output = vec![f32::NAN; CHILD_CAPACITY_FRAMES * CHANNELS];
    let result = plugin.drain(&mut output, &context).unwrap();

    assert_eq!(result.frames, DRY_DELAY_FRAMES);
    assert!(result.complete);
    assert_eq!(&output[..DRY_DELAY_FRAMES * CHANNELS], expected_tail);
    assert!(
        output[DRY_DELAY_FRAMES * CHANNELS..]
            .iter()
            .all(|sample| sample.is_nan())
    );
}

#[test]
fn prepared_process_drain_and_reset_are_free_of_heap_activity() {
    let input = dense_input(ACCEPTED_FRAMES);
    let mut plugin = make_fir_plugin(Route::Plugin, false);
    let mut process_output = vec![f32::NAN; input.len()];
    let context = ProcessContext::new(SAMPLE_RATE, ACCEPTED_FRAMES);
    let drain_context = ProcessContext::new(SAMPLE_RATE, 0);
    let declared_capacity = plugin.drain_output_frames_max();
    let mut scratch = vec![f32::NAN; declared_capacity * CHANNELS];
    let mut drained_frames = 0_usize;
    let mut completed = false;
    let mut checkpoints = [(0_usize, 0_usize); 4];

    let counts = heap_counts(|| {
        plugin
            .process(&input, &mut process_output, &context)
            .unwrap();
        checkpoints[0] = (HEAP_ALLOCATIONS.get(), HEAP_DEALLOCATIONS.get());
        plugin.begin_drain(&drain_context).unwrap();
        checkpoints[1] = (HEAP_ALLOCATIONS.get(), HEAP_DEALLOCATIONS.get());
        for _ in 0..DRAIN_CALL_LIMIT {
            let result = plugin.drain(&mut scratch, &drain_context).unwrap();
            drained_frames += result.frames;
            if result.complete {
                completed = true;
                break;
            }
        }
        let terminal = plugin.drain(&mut [], &drain_context).unwrap();
        completed &= terminal.complete;
        checkpoints[2] = (HEAP_ALLOCATIONS.get(), HEAP_DEALLOCATIONS.get());
        plugin.reset();
        checkpoints[3] = (HEAP_ALLOCATIONS.get(), HEAP_DEALLOCATIONS.get());
    });
    for (label, checkpoint) in [
        ("process", checkpoints[0]),
        ("begin drain", checkpoints[1]),
        ("full drain", checkpoints[2]),
        ("reset", checkpoints[3]),
        ("combined", counts),
    ] {
        assert_eq!(checkpoint, (0, 0), "{label} heap activity");
    }
    assert!(completed, "the bounded FIR child must reach completion");
    assert!(
        drained_frames > 0,
        "the heap-guarded route must emit its tail"
    );
}

fn drain_all_after_begin(
    plugin: &mut ABComparePlugin,
    context: &ProcessContext,
) -> (Vec<f32>, Vec<usize>) {
    let capacity_frames = plugin.drain_output_frames_max();
    let mut block = vec![f32::NAN; capacity_frames * CHANNELS];
    let mut output = Vec::new();
    let mut partition = Vec::new();
    for _ in 0..DRAIN_CALL_LIMIT {
        block.fill(f32::NAN);
        let result = plugin.drain(&mut block, context).unwrap();
        partition.push(result.frames);
        output.extend_from_slice(&block[..result.frames * CHANNELS]);
        if result.complete {
            return (output, partition);
        }
    }
    panic!("AUD137 drain exceeded the test's bounded call allowance");
}

#[test]
fn enabled_autogain_drain_matches_ordinary_zero_continuation() {
    let route = Route::Plugin;
    let path_a = path_config(route, "fir", &[0.5], 0, 1);
    let path_b = path_config(route, "fir", &[1.0, 0.25], 0, 1);
    let params = ABComparePluginParams {
        path_a,
        path_b,
        mix: 1.0,
        auto_gain_enabled: true,
        ..ABComparePluginParams::default()
    };
    let mut draining = ABComparePlugin::from_params_with_factory(
        CHANNELS,
        SAMPLE_RATE,
        params.clone(),
        fir_factory,
    )
    .unwrap();
    let mut continuation =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, fir_factory)
            .unwrap();
    draining.initialize(f64::from(SAMPLE_RATE)).unwrap();
    continuation.initialize(f64::from(SAMPLE_RATE)).unwrap();

    let input = dense_input(9_600);
    let transition_frame = 19 * 480;
    let prefix = &input[..transition_frame * CHANNELS];
    let mut initial_a = render_chunks(&mut draining, prefix, &[480; 19]);
    let mut initial_b = render_chunks(&mut continuation, prefix, &[480; 19]);
    assert_eq!(initial_a, initial_b);
    for plugin in [&mut draining, &mut continuation] {
        plugin
            .set_parameter(ParameterId::from("mix"), ParameterValue::Float(0.25))
            .unwrap();
        plugin
            .set_parameter(ParameterId::from("bypass"), ParameterValue::Bool(true))
            .unwrap();
    }
    let final_start = transition_frame * CHANNELS;
    let mut final_a = vec![f32::NAN; 480 * CHANNELS];
    let mut final_b = vec![f32::NAN; 480 * CHANNELS];
    let final_context = ProcessContext::new(SAMPLE_RATE, 480);
    draining
        .process(&input[final_start..], &mut final_a, &final_context)
        .unwrap();
    continuation
        .process(&input[final_start..], &mut final_b, &final_context)
        .unwrap();
    assert_eq!(final_a, final_b);
    initial_a.extend_from_slice(&final_a);
    initial_b.extend_from_slice(&final_b);
    assert_eq!(initial_a, initial_b);

    let context = ProcessContext::new(SAMPLE_RATE, 0);
    draining.begin_drain(&context).unwrap();
    let (drained, partitions) = drain_all_after_begin(&mut draining, &context);
    let mut continued = Vec::new();
    for &frames in &partitions {
        if frames == 0 {
            continue;
        }
        let mut block = vec![f32::NAN; frames * CHANNELS];
        continuation
            .process(
                &vec![0.0; frames * CHANNELS],
                &mut block,
                &ProcessContext::new(SAMPLE_RATE, frames),
            )
            .unwrap();
        continued.extend_from_slice(&block);
    }
    assert_eq!(drained, continued);
    assert!(drained.iter().any(|sample| sample.abs() > 1.0e-4));
}

#[test]
fn a_child_failure_after_the_other_child_advances_requires_reset() {
    COUNTED_DRAIN_CALLS.set(0);
    FAILING_DRAIN_CALLS.set(0);
    let params = ABComparePluginParams {
        path_a: PathConfig::Plugin {
            plugin_type: "counted-fir".to_owned(),
            parameters: json!({"taps": [1.0, 0.5], "drain_capacity": 1}),
        },
        path_b: PathConfig::Plugin {
            plugin_type: "fail-on-drain".to_owned(),
            parameters: json!({"taps": [1.0, -0.25], "drain_capacity": 1}),
        },
        mix: -1.0,
        auto_gain_enabled: false,
        ..ABComparePluginParams::default()
    };
    let mut plugin =
        ABComparePlugin::from_params_with_factory(CHANNELS, SAMPLE_RATE, params, fir_factory)
            .unwrap();
    plugin.initialize(f64::from(SAMPLE_RATE)).unwrap();
    let mut output = [0.0; CHANNELS];
    plugin
        .process(
            &[0.25, -0.5],
            &mut output,
            &ProcessContext::new(SAMPLE_RATE, 1),
        )
        .unwrap();
    let context = ProcessContext::new(SAMPLE_RATE, 0);
    plugin.begin_drain(&context).unwrap();
    let mut block = vec![f32::NAN; plugin.drain_output_frames_max() * CHANNELS];
    assert!(plugin.drain(&mut block, &context).is_err());
    assert!(COUNTED_DRAIN_CALLS.get() > 0);
    let counted_calls = COUNTED_DRAIN_CALLS.get();
    let failing_calls = FAILING_DRAIN_CALLS.get();
    assert!(failing_calls > 0);
    assert!(plugin.drain(&mut block, &context).is_err());
    assert_eq!(COUNTED_DRAIN_CALLS.get(), counted_calls);
    assert_eq!(FAILING_DRAIN_CALLS.get(), failing_calls);
    assert!(
        plugin
            .process(
                &[0.0; CHANNELS],
                &mut [f32::NAN; CHANNELS],
                &ProcessContext::new(SAMPLE_RATE, 1),
            )
            .is_err()
    );

    plugin.reset();
    assert_eq!(
        plugin
            .process(
                &[0.25, -0.5],
                &mut output,
                &ProcessContext::new(SAMPLE_RATE, 1),
            )
            .unwrap(),
        1
    );
}

#[test]
fn outer_daw_host_returns_the_same_complete_abcompare_tail() {
    let input = dense_input(ACCEPTED_FRAMES);
    let expected = reference_default(&input);
    let plugin = make_fir_plugin(Route::Plugin, false);
    let mut host = DawHost::new(CHANNELS, SAMPLE_RATE);
    host.add_plugin(Box::new(plugin)).unwrap();
    host.build().unwrap();

    let mut process_output = vec![0.0; input.len()];
    host.process(&input, &mut process_output).unwrap();
    let capacity_frames = host.drain_output_frames_max();
    let mut block = vec![f32::NAN; capacity_frames * CHANNELS];
    let mut tail = Vec::new();
    for _ in 0..DRAIN_CALL_LIMIT {
        block.fill(f32::NAN);
        let result = host.drain(&mut block).unwrap();
        tail.extend_from_slice(&block[..result.frames * CHANNELS]);
        if result.complete {
            let mut actual = process_output;
            actual.extend_from_slice(&tail);
            assert_vectors_close(&actual, &expected, "outer DawHost route");
            return;
        }
    }
    panic!("outer DawHost did not finish the ABCompare tail");
}
