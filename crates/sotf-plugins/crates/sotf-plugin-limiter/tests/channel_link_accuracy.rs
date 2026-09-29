// Rust guideline compliant 2026-02-21

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::parametric_in_place_plugin::ParametricInPlacePlugin;
use sotf_host::plugin::ProcessContext;
use sotf_plugin_limiter::{LimiterPlugin, LimiterPluginParams};

fn limiter(channels: usize, sample_rate: u32, lookahead_ms: f32, link: f32) -> LimiterPlugin {
    let mut plugin = LimiterPlugin::from_params(
        channels,
        LimiterPluginParams {
            oversampling: 0,
            threshold_db: -12.0,
            release_ms: 20.0,
            lookahead_ms,
            dual_release: true,
            link_amount: link,
            soft: false,
            true_peak: false,
            isp_mode: false,
            mix: 1.0,
            feed_forward: false,
        },
    );
    plugin.initialize(sample_rate).unwrap();
    plugin
}

fn process_blocks(
    plugin: &mut LimiterPlugin,
    samples: &mut [f32],
    sample_rate: u32,
    frames: usize,
) {
    let channels = plugin.channels();
    for block in samples.chunks_mut(frames * channels) {
        plugin
            .process_in_place(
                block,
                &ProcessContext::new(sample_rate, block.len() / channels),
            )
            .unwrap();
    }
}

#[test]
fn linked_dual_release_matches_mono_for_every_channel_and_block_size() {
    for sample_rate in [44_100, 48_000, 96_000] {
        // A sustained overload followed by a quiet tail exercises both release
        // time constants. The final partial block must continue the same state.
        let input: Vec<f32> = (0..sample_rate as usize / 2 + 17)
            .map(|frame| {
                if frame < sample_rate as usize / 10 {
                    1.0
                } else {
                    0.125
                }
            })
            .collect();
        for lookahead_ms in [0.0, 5.0] {
            let mut mono = limiter(1, sample_rate, lookahead_ms, 1.0);
            let mut expected = input.clone();
            process_blocks(&mut mono, &mut expected, sample_rate, 257);

            for channels in [2, 6] {
                for block_frames in [1, 127, 1024] {
                    let mut plugin = limiter(channels, sample_rate, lookahead_ms, 1.0);
                    let mut output: Vec<f32> = input
                        .iter()
                        .flat_map(|&sample| std::iter::repeat_n(sample, channels))
                        .collect();
                    process_blocks(&mut plugin, &mut output, sample_rate, block_frames);

                    for (frame, (&reference, actual)) in expected
                        .iter()
                        .zip(output.chunks_exact(channels))
                        .enumerate()
                    {
                        for (channel, &sample) in actual.iter().enumerate() {
                            assert_eq!(
                                sample, reference,
                                "rate={sample_rate}, lookahead={lookahead_ms}, channels={channels}, block={block_frames}, frame={frame}, channel={channel}"
                            );
                        }
                    }
                }
            }
        }
    }
}

#[test]
fn linked_dual_release_preserves_channel_ratios_during_release() {
    let sample_rate = 48_000;
    let scales = [1.0, -0.5, 0.25, -0.125, 0.0625, -0.03125];
    let mut plugin = limiter(scales.len(), sample_rate, 5.0, 1.0);
    let mut output: Vec<f32> = (0..24_017)
        .flat_map(|frame| {
            let sample = if frame < 4800 { 1.0 } else { 0.125 };
            scales.map(|scale| sample * scale)
        })
        .collect();
    process_blocks(&mut plugin, &mut output, sample_rate, 127);
    for frame in output.chunks_exact(scales.len()).skip(5040) {
        for (&sample, scale) in frame.iter().zip(scales) {
            assert_eq!(
                sample,
                frame[0] * scale,
                "linked release changed channel ratio"
            );
        }
    }
}

#[test]
fn enabling_full_linking_merges_existing_independent_envelopes() {
    let sample_rate = 48_000;
    let mut plugin = limiter(2, sample_rate, 0.0, 0.0);
    let mut overload: Vec<f32> = (0..4800).flat_map(|_| [0.125, 1.0]).collect();
    process_blocks(&mut plugin, &mut overload, sample_rate, 128);
    let previous_gain = overload[overload.len() - 1];
    plugin
        .set_parameter(ParameterId::from("link_amount"), ParameterValue::Float(1.0))
        .unwrap();

    let mut quiet = vec![0.125; 1024];
    process_blocks(&mut plugin, &mut quiet, sample_rate, 127);
    for frame in quiet.as_chunks::<2>().0 {
        assert_eq!(
            frame[0], frame[1],
            "full linking retained different gain histories"
        );
    }
    assert!(
        quiet[0] / 0.125 < previous_gain + 0.001,
        "linking must retain the more attenuated channel's release envelope"
    );
}

#[test]
fn independent_dual_release_matches_separate_mono_instances() {
    let sample_rate = 48_000;
    for lookahead_ms in [0.0, 5.0] {
        let mut inputs: [Vec<f32>; 2] = std::array::from_fn(|channel| {
            (0..24_017)
                .map(|frame| {
                    let onset = channel * 6000;
                    if (onset..onset + 4000).contains(&frame) {
                        1.0
                    } else {
                        0.125
                    }
                })
                .collect()
        });
        let mut interleaved: Vec<f32> = inputs[0]
            .iter()
            .zip(&inputs[1])
            .flat_map(|(&left, &right)| [left, right])
            .collect();
        for mono_input in &mut inputs {
            let mut mono = limiter(1, sample_rate, lookahead_ms, 0.0);
            process_blocks(&mut mono, mono_input, sample_rate, 257);
        }
        let mut stereo = limiter(2, sample_rate, lookahead_ms, 0.0);
        process_blocks(&mut stereo, &mut interleaved, sample_rate, 127);
        for (frame, actual) in interleaved.as_chunks::<2>().0.iter().enumerate() {
            assert_eq!(
                actual,
                &[inputs[0][frame], inputs[1][frame]],
                "independent envelopes cross-coupled at frame={frame}, lookahead={lookahead_ms}"
            );
        }
    }
}

#[test]
fn isp_stage_preserves_linking_and_independent_streaming_across_blocks() {
    for sample_rate in [48_000, 96_000, 192_000] {
        for link in [0.0, 1.0] {
            let make_plugin = |channels| {
                let params: LimiterPluginParams = serde_json::from_value(serde_json::json!({
                    "threshold_db": -6.0,
                    "release_ms": 10.0,
                    "lookahead_ms": 0.13,
                    "isp_mode": true,
                    "dual_release": true,
                    "link_amount": link,
                }))
                .unwrap();
                let mut plugin = LimiterPlugin::from_params(channels, params);
                plugin.initialize(sample_rate).unwrap();
                plugin
            };
            let inputs: [Vec<f32>; 2] = std::array::from_fn(|channel| {
                (0..8192)
                    .map(|frame| {
                        let phase = if link == 0.0 {
                            channel as f64 * 0.375
                        } else {
                            0.0
                        };
                        if frame % 274 < 137 {
                            (1.2 * (std::f64::consts::TAU
                                * (12_000.0 * frame as f64 / sample_rate as f64 + phase))
                                .sin()) as f32
                        } else {
                            0.0
                        }
                    })
                    .collect()
            });
            let mut references = inputs.clone();
            for reference in &mut references {
                let mut mono = make_plugin(1);
                process_blocks(&mut mono, reference, sample_rate, 257);
            }
            for frames in [1, 127, 1024] {
                let mut output: Vec<f32> = inputs[0]
                    .iter()
                    .zip(&inputs[1])
                    .flat_map(|(&left, &right)| [left, right])
                    .collect();
                let mut stereo = make_plugin(2);
                process_blocks(&mut stereo, &mut output, sample_rate, frames);
                for (frame, actual) in output.as_chunks::<2>().0.iter().enumerate() {
                    assert_eq!(
                        actual,
                        &[references[0][frame], references[1][frame]],
                        "ISP linking/streaming mismatch: rate={sample_rate}, link={link}, block={frames}, frame={frame}"
                    );
                }
            }
        }
    }
}
