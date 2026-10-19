//! Exercise the final ramp samples through the public host adapter.

// Rust guideline compliant 2026-02-21
use sotf_host::parametric_plugin::ParametricPluginAdapter;
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext};
use sotf_plugin_gain::GainPlugin;

fn render(rate: u32, channels: usize, per_channel: bool, partitions: &[usize]) -> Vec<f32> {
    let gain = if per_channel {
        GainPlugin::new_per_channel(vec![0.0; channels]).unwrap()
    } else {
        GainPlugin::new(channels, 0.0)
    };
    let mut processor = ParametricPluginAdapter::new(gain);
    processor.initialize(f64::from(rate)).unwrap();
    let frames = 30_000;
    let input = vec![1.0; frames * channels];
    let mut output = vec![0.0; input.len()];
    let mut position = 0;
    let mut block = 0;
    while position < frames {
        if position == 0 || position == 14_003 {
            if per_channel {
                for channel in 0..channels {
                    let db = if position == 0 { -60.0 } else { -36.0 } + channel as f32;
                    processor
                        .set_parameter(
                            ParameterId::from(format!("gain_db_{channel}")),
                            ParameterValue::Float(db),
                        )
                        .unwrap();
                }
            } else {
                processor
                    .set_parameter(
                        ParameterId::from("gain_db"),
                        ParameterValue::Float(if position == 0 { -60.0 } else { -36.0 }),
                    )
                    .unwrap();
            }
        }
        let boundary = if position < 14_003 { 14_003 } else { frames };
        let count = partitions[block % partitions.len()].min(boundary - position);
        let range = position * channels..(position + count) * channels;
        processor
            .process(
                &input[range.clone()],
                &mut output[range],
                &ProcessContext::new(rate, count),
            )
            .unwrap();
        position += count;
        block += 1;
    }
    output
}

#[test]
fn settling_and_retargeting_use_absolute_samples_for_scalar_and_simd_layouts() {
    for rate in [44_100, 48_000, 96_000] {
        for channels in [1, 2, 7] {
            for per_channel in [false, true] {
                let reference = render(rate, channels, per_channel, &[1]);
                assert!(reference[0] > 0.9 && reference[14_002 * channels] < 0.002);
                for pattern in [&[8192][..], &[7, 31, 257, 1024][..]] {
                    let actual = render(rate, channels, per_channel, pattern);
                    let error = actual
                        .iter()
                        .zip(&reference)
                        .map(|(a, b)| (a - b).abs())
                        .fold(0.0_f32, f32::max);
                    assert_eq!(
                        error, 0.0,
                        "rate={rate}, channels={channels}, per_channel={per_channel}, pattern={pattern:?}"
                    );
                }
            }
        }
    }
}
