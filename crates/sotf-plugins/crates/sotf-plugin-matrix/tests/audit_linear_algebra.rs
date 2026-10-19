//! Independent rectangular products and absolute-sample route automation.

// Rust guideline compliant 2026-02-21
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_matrix::MatrixPlugin;

#[test]
fn rectangular_dense_sparse_zero_and_tiny_rows_match_f64_products() {
    for (inputs, outputs) in [(1, 3), (3, 1), (3, 5), (7, 11), (31, 17), (128, 3)] {
        let mut matrix: Vec<f32> = (0..inputs * outputs)
            .map(|i| {
                if i % 5 == 0 {
                    0.0
                } else {
                    ((i * 17 % 31) as f32 - 15.0) / 16.0
                }
            })
            .collect();
        if outputs > 1 {
            matrix[..inputs].fill(0.0);
            matrix[(outputs - 1) * inputs..].fill(0.0);
            matrix[(outputs - 1) * inputs + inputs - 1] = 1e-8;
        }
        let mut plugin = MatrixPlugin::with_matrix(inputs, outputs, matrix.clone()).unwrap();
        plugin.initialize(48_000.0).unwrap();
        let frames = 73;
        let input: Vec<f32> = (0..frames * inputs)
            .map(|i| ((i * 13 % 37) as f32 - 18.0) / 19.0)
            .collect();
        let mut output = vec![f32::NAN; frames * outputs];
        plugin
            .process(&input, &mut output, &ProcessContext::new(48_000, frames))
            .unwrap();
        for frame in 0..frames {
            for out in 0..outputs {
                let mut expected = 0.0_f64;
                let mut magnitude = 0.0;
                for channel in 0..inputs {
                    let term = f64::from(input[frame * inputs + channel])
                        * f64::from(matrix[out * inputs + channel]);
                    expected += term;
                    magnitude += term.abs();
                }
                let error = (f64::from(output[frame * outputs + out]) - expected).abs();
                assert!(
                    error <= magnitude * 2e-6 + 1e-14,
                    "{inputs}x{outputs}, output={out}: error={error}, expected={expected}"
                );
            }
        }
    }
}

#[test]
fn sparse_channel_maps_preserve_logical_order_and_clear_unrouted_outputs() {
    let input_map = [7, 2, 0];
    let output_map = [9, 3];
    let matrix = [0.25, -0.5, 1.0, -1.0, 0.75, 0.0];
    let mut plugin =
        MatrixPlugin::with_sparse_mapping(input_map.to_vec(), output_map.to_vec(), matrix.to_vec())
            .unwrap();
    plugin.initialize(96_000.0).unwrap();
    let input: Vec<f32> = (0..8 * 17)
        .map(|i| (i % 8 + 1) as f32 * 0.125 + (i / 8) as f32)
        .collect();
    let mut output = vec![f32::NAN; 10 * 17];
    plugin
        .process(&input, &mut output, &ProcessContext::new(96_000, 17))
        .unwrap();
    for frame in 0..17 {
        for physical in 0..10 {
            let expected = if let Some(logical) = output_map.iter().position(|&ch| ch == physical) {
                input_map
                    .iter()
                    .enumerate()
                    .map(|(i, &ch)| {
                        f64::from(input[frame * 8 + ch]) * f64::from(matrix[logical * 3 + i])
                    })
                    .sum()
            } else {
                0.0
            };
            assert!((f64::from(output[frame * 10 + physical]) - expected).abs() < 1e-6);
        }
    }
}

fn automated(rate: u32, pattern: &[usize]) -> Vec<f32> {
    let mut plugin = MatrixPlugin::with_matrix(1, 1, vec![1.0]).unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    let frames = 24_000;
    let input = vec![1.0; frames];
    let mut output = vec![0.0; frames];
    let events = [(0, 0.0), (4097, 0.3), (8193, 0.0), (17001, 1.0)];
    let mut event = 0;
    let mut position = 0;
    let mut block = 0;
    while position < frames {
        if event < events.len() && events[event].0 == position {
            plugin.set_gain(0, 0, events[event].1).unwrap();
            event += 1;
        }
        let boundary = events.get(event).map_or(frames, |e| e.0);
        let count = pattern[block % pattern.len()].min(boundary - position);
        plugin
            .process(
                &input[position..position + count],
                &mut output[position..position + count],
                &ProcessContext::new(rate, count),
            )
            .unwrap();
        position += count;
        block += 1;
    }
    output
}

#[test]
fn fading_routes_retire_and_reactivate_at_the_same_source_samples() {
    for rate in [44_100, 48_000, 96_000] {
        let reference = automated(rate, &[1]);
        for pattern in [&[8192][..], &[7, 31, 257][..]] {
            let actual = automated(rate, pattern);
            let error = actual
                .iter()
                .zip(&reference)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0_f32, f32::max);
            assert_eq!(
                error, 0.0,
                "rate={rate}, pattern={pattern:?}: fade depends on callbacks"
            );
        }
    }
}

#[test]
fn coherent_overload_is_the_actual_linear_sum() {
    let mut plugin = MatrixPlugin::with_matrix(7, 1, vec![1.0; 7]).unwrap();
    plugin.initialize(48_000.0).unwrap();
    let mut output = vec![0.0; 31];
    plugin
        .process(
            &vec![1.0; 31 * 7],
            &mut output,
            &ProcessContext::new(48_000, 31),
        )
        .unwrap();
    assert!(
        output.iter().all(|&x| x == 7.0),
        "routing must preserve coherent headroom responsibility"
    );
}

#[test]
fn even_sub_epsilon_route_changes_reach_their_exact_targets() {
    for (start, target) in [(0.0, 1e-8), (0.0, 1.0)] {
        let mut plugin = MatrixPlugin::with_matrix(1, 1, vec![start]).unwrap();
        plugin.initialize(96_000.0).unwrap();
        plugin.set_gain(0, 0, target).unwrap();
        let input = vec![1.0; 9600];
        let mut output = vec![0.0; input.len()];
        plugin
            .process(
                &input,
                &mut output,
                &ProcessContext::new(96_000, input.len()),
            )
            .unwrap();
        assert_eq!(
            *output.last().unwrap(),
            target,
            "the small target or the tail of its smoother was frozen"
        );
    }
}
