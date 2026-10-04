//! Independent correlation accuracy and accepted-callback integrity.

// Rust guideline compliant 2026-02-21
use sotf_host::analyzer::CorrelationData;
use sotf_host::plugin::PluginCompiledOp;
use sotf_host::{ChannelCorrelationPlugin, ParameterId, ParameterValue, Plugin, ProcessContext};

fn signal(frames: usize, channels: usize, rate: u32) -> Vec<f32> {
    (0..frames)
        .flat_map(|frame| {
            let time = frame as f64 / f64::from(rate);
            let a = (std::f64::consts::TAU * 997.0 * time).sin();
            let b = (std::f64::consts::TAU * 619.0 * time).sin();
            (0..channels).map(move |channel| {
                let value = match channel % 4 {
                    0 => a,
                    1 => -a,
                    2 => b,
                    _ => a + 0.7 * b,
                };
                (value + channel as f64 * 0.13) as f32
            })
        })
        .collect()
}

fn snapshot(plugin: &ChannelCorrelationPlugin) -> (u64, Vec<f32>) {
    let data = plugin.get_data().unwrap();
    let data = data.downcast_ref::<CorrelationData>().unwrap();
    (data.samples_seen, data.matrix.as_ref().clone())
}

// Independently sum the finite weighted sample set, newest first. The documented
// 400-ms exponential window supplies the weights; no production accumulator or
// triangular-index helper is used to generate the expected centered covariance.
fn pearson(input: &[f32], channels: usize, rate: u32) -> Vec<f64> {
    let decay = 1.0 - 1.0 / (0.4 * f64::from(rate));
    let mut weight = 1.0;
    let mut mass = 0.0;
    let mut sums = vec![0.0; channels];
    let mut products = vec![0.0; channels * channels];
    for frame in input.chunks_exact(channels).rev() {
        mass += weight;
        for i in 0..channels {
            let x = f64::from(frame[i]);
            sums[i] += weight * x;
            for j in 0..channels {
                products[i * channels + j] += weight * x * f64::from(frame[j]);
            }
        }
        weight *= decay;
    }
    let variances: Vec<_> = (0..channels)
        .map(|i| products[i * channels + i] - sums[i] * sums[i] / mass)
        .collect();
    (0..channels * channels)
        .map(|index| {
            let (i, j) = (index / channels, index % channels);
            (products[index] - sums[i] * sums[j] / mass) / (variances[i] * variances[j]).sqrt()
        })
        .collect()
}

fn feed(
    plugin: &mut ChannelCorrelationPlugin,
    input: &[f32],
    rate: u32,
    pattern: &[usize],
    compiled: bool,
) {
    let channels = plugin.input_channels();
    let mut output = vec![f32::NAN; pattern.iter().max().unwrap() * channels];
    let (mut offset, mut call) = (0, 0);
    while offset < input.len() {
        let count = (pattern[call % pattern.len()] * channels).min(input.len() - offset);
        let block = &input[offset..offset + count];
        let destination = &mut output[..count];
        destination.fill(f32::NAN);
        let context = ProcessContext::new(rate, count / channels);
        let actual = if compiled {
            plugin
                .process_compiled_f32(PluginCompiledOp::AnalyzerTap, block, destination, &context)
                .unwrap()
        } else {
            plugin.process(block, destination, &context)
        };
        assert_eq!(actual.unwrap(), count / channels);
        assert_eq!(destination, block);
        offset += count;
        call += 1;
    }
}

#[test]
fn oversized_and_irregular_callbacks_preserve_every_frame_and_centered_correlation() {
    for rate in [48_000, 96_000] {
        for channels in [2, 7, 32] {
            let input = signal(28_000, channels, rate);
            let expected = pearson(&input, channels, rate);
            let mut reference = None;
            for compiled in [false, true] {
                for pattern in [&[20_000, 8_000][..], &[1, 137, 8_193, 17][..]] {
                    let mut plugin = ChannelCorrelationPlugin::new(channels).unwrap();
                    plugin.initialize(f64::from(rate)).unwrap();
                    feed(&mut plugin, &input, rate, pattern, compiled);
                    let actual = snapshot(&plugin);
                    assert_eq!(actual.0, 28_000, "{rate} Hz, {channels} channels");
                    let max_error = actual
                        .1
                        .iter()
                        .zip(&expected)
                        .map(|(a, b)| (f64::from(*a) - b).abs())
                        .fold(0.0_f64, f64::max);
                    assert!(max_error < 2.0e-6, "independent Pearson error {max_error}");
                    assert!((actual.1[1] + 1.0).abs() < 1.0e-6);
                    if let Some(reference) = &reference {
                        assert_eq!(&actual, reference, "partition/compiled parity");
                    } else {
                        reference = Some(actual);
                    }
                }
            }
        }
    }
}

#[test]
fn invalid_callbacks_preserve_output_and_subsequent_measurements() {
    for enabled in [false, true] {
        for case in 0..10 {
            let mut plugin = ChannelCorrelationPlugin::new(7).unwrap();
            let mut reference = ChannelCorrelationPlugin::new(7).unwrap();
            let warm = signal(1_024, 7, 48_000);
            for instance in [&mut plugin, &mut reference] {
                feed(instance, &warm, 48_000, &[137], false);
                instance
                    .set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(enabled))
                    .unwrap();
            }
            let before = snapshot(&plugin);
            let mut input = signal(17, 7, 48_000);
            let mut output = vec![999.0; input.len()];
            let mut context = ProcessContext::new(48_000, 17);
            match case {
                0 => context.num_frames = 16,
                1 => context.sample_rate = 24_000.0,
                2 => context.sample_rate = 0.0,
                3 => *input.last_mut().unwrap() = f32::NAN,
                4 => *input.last_mut().unwrap() = f32::INFINITY,
                5 => *input.last_mut().unwrap() = f32::NEG_INFINITY,
                6 => {
                    input.pop();
                }
                7 => {
                    output.pop();
                }
                8 => output.push(999.0),
                _ => context.num_frames = usize::MAX,
            }
            let sentinel = output.clone();
            assert!(
                plugin.process(&input, &mut output, &context).is_err(),
                "case{case}"
            );
            assert_eq!(output, sentinel);
            assert_eq!(snapshot(&plugin), before);
            let next = signal(257, 7, 48_000);
            feed(&mut plugin, &next, 48_000, &[257], false);
            feed(&mut reference, &next, 48_000, &[257], false);
            assert_eq!(snapshot(&plugin), snapshot(&reference));
        }
    }
}

#[test]
fn reset_reinitialize_disabled_and_empty_calls_preserve_the_analysis_clock() {
    let input = signal(20_000, 7, 48_000);
    let mut plugin = ChannelCorrelationPlugin::new(7).unwrap();
    feed(&mut plugin, &input, 48_000, &[20_000], false);
    let original = snapshot(&plugin);
    assert!(plugin.initialize(0.0).is_err());
    assert_eq!(snapshot(&plugin), original);
    assert_eq!(
        plugin
            .process(&[], &mut [], &ProcessContext::new(48_000, 0))
            .unwrap(),
        0
    );
    assert_eq!(snapshot(&plugin), original);
    plugin
        .set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(false))
        .unwrap();
    feed(&mut plugin, &input, 48_000, &[20_000], true);
    assert_eq!(snapshot(&plugin), original);
    plugin
        .set_parameter(ParameterId::from("enabled"), ParameterValue::Bool(true))
        .unwrap();
    plugin.reset();
    let cold = snapshot(&plugin);
    assert_eq!(cold.0, 0);
    for (index, value) in cold.1.iter().enumerate() {
        assert_eq!(*value, if index / 7 == index % 7 { 1.0 } else { 0.0 });
    }
    feed(&mut plugin, &input, 48_000, &[137, 8_193], false);
    assert_eq!(snapshot(&plugin), original);
    plugin.initialize(96_000.0).unwrap();
    let mut fresh = ChannelCorrelationPlugin::new(7).unwrap();
    fresh.initialize(96_000.0).unwrap();
    feed(&mut plugin, &input, 96_000, &[20_000], false);
    feed(&mut fresh, &input, 96_000, &[20_000], false);
    assert_eq!(snapshot(&plugin), snapshot(&fresh));
}

#[test]
fn invalid_construction_is_rejected_before_preparing_matrix_storage() {
    assert!(ChannelCorrelationPlugin::new(0).is_err());
    assert!(ChannelCorrelationPlugin::new(usize::MAX).is_err());
}
