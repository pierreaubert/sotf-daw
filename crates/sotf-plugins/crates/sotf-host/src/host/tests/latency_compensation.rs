use super::super::daw_host::DawHost;
use super::super::graph_edge::GraphEdge;
use crate::parameters::{Parameter, ParameterId, ParameterValue};
use crate::plugin::{Plugin, PluginInfo, ProcessContext};
use std::collections::VecDeque;

/// Reference delay independent of the host's delay-buffer implementation.
struct DelayedPlugin {
    channels: usize,
    latency: usize,
    history: VecDeque<f64>,
}

impl DelayedPlugin {
    fn new(channels: usize, latency: usize) -> Self {
        Self {
            channels,
            latency,
            history: VecDeque::from(vec![0.0; channels * latency]),
        }
    }

    fn next_sample(&mut self, input: f64) -> f64 {
        if self.latency == 0 {
            return input;
        }
        let output = self.history.pop_front().unwrap();
        self.history.push_back(input);
        output
    }
}

impl Plugin for DelayedPlugin {
    fn info(&self) -> PluginInfo {
        PluginInfo::new("Reference delay", "0.1", "test")
    }

    fn input_channels(&self) -> usize {
        self.channels
    }

    fn output_channels(&self) -> usize {
        self.channels
    }

    fn parameters(&self) -> Vec<Parameter> {
        Vec::new()
    }

    fn set_parameter(&mut self, _: ParameterId, _: ParameterValue) -> Result<(), String> {
        Err("reference delay has no parameters".into())
    }

    fn get_parameter(&self, _: &ParameterId) -> Option<ParameterValue> {
        None
    }

    fn process(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        for (output, &input) in output.iter_mut().zip(input) {
            *output = self.next_sample(f64::from(input)) as f32;
        }
        Ok(context.num_frames)
    }

    fn supports_f64(&self) -> bool {
        true
    }

    fn process_f64(
        &mut self,
        input: &[f64],
        output: &mut [f64],
        context: &ProcessContext,
    ) -> Result<usize, String> {
        for (output, &input) in output.iter_mut().zip(input) {
            *output = self.next_sample(input);
        }
        Ok(context.num_frames)
    }

    fn latency_samples(&self) -> usize {
        self.latency
    }

    fn reset(&mut self) {
        self.history.iter_mut().for_each(|sample| *sample = 0.0);
    }
}

// Three copies of the input reach two terminal nodes. The common root adds
// two frames; the plugin merge aligns the 3- and 7-frame paths, and the host
// output must then align that 9-frame path with the 13-frame terminal path.
fn delayed_graph(channels: usize) -> DawHost {
    let mut host = DawHost::new(channels, 48_000);
    let root = host
        .add_node("root".into(), Box::new(DelayedPlugin::new(channels, 2)))
        .unwrap();
    let long = host
        .add_node("long".into(), Box::new(DelayedPlugin::new(channels, 11)))
        .unwrap();
    let short = host
        .add_node("short".into(), Box::new(DelayedPlugin::new(channels, 3)))
        .unwrap();
    let medium = host
        .add_node("medium".into(), Box::new(DelayedPlugin::new(channels, 7)))
        .unwrap();
    let merge = host
        .add_node("merge".into(), Box::new(DelayedPlugin::new(channels, 0)))
        .unwrap();
    for (from, to) in [
        (root, long),
        (root, short),
        (root, medium),
        (short, merge),
        (medium, merge),
    ] {
        host.add_edge(GraphEdge::new(from, to)).unwrap();
    }
    host.build().unwrap();
    assert_eq!(host.total_latency_samples(), 13);
    host
}

fn render(host: &mut DawHost, input: &[f64], block_sizes: &[usize], native_f64: bool) -> Vec<f64> {
    let channels = host.input_channels();
    let mut cursor = 0;
    let mut rendered = Vec::new();
    for &frames in block_sizes {
        let samples = frames * channels;
        let block = &input[cursor..cursor + samples];
        if native_f64 {
            let mut output = vec![f64::NAN; samples];
            assert_eq!(host.process_f64(block, &mut output).unwrap(), frames);
            rendered.extend(output);
        } else {
            let input: Vec<f32> = block.iter().map(|&sample| sample as f32).collect();
            let mut output = vec![f32::NAN; samples];
            assert_eq!(host.process(&input, &mut output).unwrap(), frames);
            rendered.extend(output.into_iter().map(f64::from));
        }
        cursor += samples;
    }
    assert_eq!(cursor, input.len());
    rendered
}

fn check_graph_output_alignment(native_f64: bool) {
    for channels in [1, 2, 6] {
        let mut input = vec![0.0; 64 * channels];
        for frame in 0..40 {
            for channel in 0..channels {
                // Exact binary fractions expose channel shifts and preserve f32 equality.
                input[frame * channels + channel] =
                    ((frame * 7 + channel * 3) % 17) as f64 / 32.0 - 0.25;
            }
        }
        if native_f64 {
            // This value is lost by an f32 bridge; retain it in native processing.
            input[0] = 1.0 + 2.0_f64.powi(-40);
        }
        let mut expected = vec![0.0; input.len()];
        for (output, &input) in expected[13 * channels..].iter_mut().zip(&input) {
            *output = input * 3.0;
        }
        for blocks in [&[64][..], &[1, 4, 3, 11, 2, 43][..]] {
            let mut host = delayed_graph(channels);
            let output = render(&mut host, &input, blocks, native_f64);
            assert_eq!(output, expected, "channels={channels}, blocks={blocks:?}");
        }
    }
}

fn check_graph_reset(native_f64: bool) {
    let channels = 2;
    let mut host = delayed_graph(channels);
    // Fill both the edge delay and terminal delay, then reset while audio remains.
    render(&mut host, &[1.0; 20], &[10], native_f64);
    host.reset();
    let silence = render(&mut host, &[0.0; 64], &[1, 5, 2, 24], native_f64);
    assert_eq!(silence, [0.0; 64]);

    // The reset graph must again produce the full, aligned impulse response.
    let mut impulse = [0.0; 64];
    impulse[0] = 1.0;
    impulse[1] = -0.5;
    let output = render(&mut host, &impulse, &[7, 3, 22], native_f64);
    let mut expected = [0.0; 64];
    expected[26] = 3.0;
    expected[27] = -1.5;
    assert_eq!(output, expected);
}

#[test]
fn graph_terminal_latency_compensation_f32_matches_reference() {
    check_graph_output_alignment(false);
}

#[test]
fn graph_terminal_latency_compensation_f64_matches_reference() {
    check_graph_output_alignment(true);
}

#[test]
fn graph_latency_compensation_reset_clears_f32_history() {
    check_graph_reset(false);
}

#[test]
fn graph_latency_compensation_reset_clears_f64_history() {
    check_graph_reset(true);
}

#[test]
fn graph_terminal_latency_compensation_rebuild_respects_bypass() {
    for native_f64 in [false, true] {
        let mut host = DawHost::new(1, 48_000);
        host.add_node("dry".into(), Box::new(DelayedPlugin::new(1, 0)))
            .unwrap();
        let delayed = host
            .add_node("delayed".into(), Box::new(DelayedPlugin::new(1, 7)))
            .unwrap();
        host.build().unwrap();
        assert_eq!(host.total_latency_samples(), 7);

        host.bypass_node(delayed).unwrap();
        host.build().unwrap();
        assert_eq!(host.total_latency_samples(), 0);
        assert_eq!(
            render(&mut host, &[0.25; 12], &[3, 9], native_f64),
            [0.5; 12]
        );

        host.unbypass_node(delayed).unwrap();
        host.build().unwrap();
        host.reset();
        assert_eq!(host.total_latency_samples(), 7);
        let output = render(&mut host, &[0.25; 12], &[3, 9], native_f64);
        assert_eq!(
            output,
            [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.5, 0.5, 0.5]
        );
    }
}
