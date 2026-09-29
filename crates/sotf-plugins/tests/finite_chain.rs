//! Real factory-created plugins preserve final program through causal host drain.

use sotf_plugins::{DawHost, create_plugin};

fn chain(rate: u32, channels: usize, phase: usize, bypass: Option<usize>) -> DawHost {
    let mut host = DawHost::new(channels, rate);
    for (kind, settings) in [
        ("gate", serde_json::json!({"lookahead_ms":1.0,"mix":0.0})),
        (
            "linear_phase_eq",
            serde_json::json!({"fir_length_index":0,"phase_mode_index":phase,"mix":0.0}),
        ),
        (
            "limiter",
            serde_json::json!({"lookahead_ms":2.0,"mix":0.0,"true_peak":false,"isp_mode":false}),
        ),
    ] {
        host.add_plugin(create_plugin(kind, &settings, channels, rate).unwrap())
            .unwrap();
    }
    if let Some(index) = bypass {
        host.bypass_plugin(index).unwrap();
    }
    host
}

fn finite_output(host: &mut DawHost, input: &[f32], channels: usize, block: usize) -> Vec<f32> {
    let mut result = Vec::new();
    for input in input.chunks(block * channels) {
        let mut output = vec![123.0; input.len()];
        let frames = host.process(input, &mut output).unwrap();
        assert_eq!(frames, input.len() / channels);
        result.extend(output);
    }
    let capacity = host.drain_output_frames_max().max(1);
    for _ in 0..4096 {
        let mut output = vec![123.0; capacity * channels];
        let drain = host.drain(&mut output).unwrap();
        assert!(drain.frames <= capacity);
        result.extend_from_slice(&output[..drain.frames * channels]);
        if drain.complete {
            assert_eq!(host.drain(&mut output).unwrap().frames, 0);
            return result;
        }
    }
    panic!("finite three-plugin chain did not finish");
}

#[test]
fn factory_chain_drains_each_delayed_stage_once_and_respects_bypass() {
    for rate in [48000, 96000] {
        for channels in [1, 2, 6] {
            for phase in 0..2 {
                for bypass in [None, Some(0), Some(1), Some(2)] {
                    let gate = if bypass == Some(0) {
                        0
                    } else {
                        rate as usize / 1000
                    };
                    let fir_delay = if bypass == Some(1) {
                        0
                    } else if phase == 0 {
                        544
                    } else {
                        32
                    };
                    let limiter = if bypass == Some(2) {
                        0
                    } else {
                        rate as usize / 500
                    };
                    let delay = gate + fir_delay + limiter;
                    let support = gate + limiter + if bypass == Some(1) { 0 } else { 1055 };
                    let input: Vec<f32> = (0..73 * channels)
                        .map(|i| {
                            if i / channels == 72 {
                                -0.125
                            } else {
                                ((i * 7 % 15) as f32 - 7.0) / 64.0
                            }
                        })
                        .collect();
                    let mut host = chain(rate, channels, phase, bypass);
                    for block in [1, 73] {
                        let actual = finite_output(&mut host, &input, channels, block);
                        assert_eq!(actual.len(), (73 + support) * channels);
                        for (index, &value) in actual.iter().enumerate() {
                            let expected = index
                                .checked_sub(delay * channels)
                                .and_then(|i| input.get(i))
                                .copied()
                                .unwrap_or(0.0);
                            assert_eq!(
                                value, expected,
                                "{rate}/{channels}/phase{phase}/bypass{bypass:?}/block{block}, sample{index}"
                            );
                        }
                        host.reset();
                    }
                }
            }
        }
    }
}

#[test]
fn speech_downmix_limiter_chain_preserves_full_program_and_composed_support() {
    const RATE: u32 = 48000;
    const CHANNELS: usize = 2;
    // Model+queue, spectral downmix, and dry limiter lookahead, independently specified.
    const DELAY: usize = 960 + 2048 + 240;
    for source_frames in [1, 73, 1023, 2049] {
        for bypass in [None, Some(0), Some(1), Some(2)] {
            let mut host = DawHost::new(CHANNELS, RATE);
            for (kind, config) in [
                ("speech_denoiser", serde_json::json!({"enabled":false})),
                (
                    "downmix",
                    serde_json::json!({"input_channels":2,"input_layout":"2.0","phase_coherence":true,"matrix_ltrt":false}),
                ),
                (
                    "limiter",
                    serde_json::json!({"lookahead_ms":5.0,"mix":0.0,"true_peak":false,"isp_mode":false}),
                ),
            ] {
                host.add_plugin(create_plugin(kind, &config, CHANNELS, RATE).unwrap())
                    .unwrap();
            }
            if let Some(index) = bypass {
                host.bypass_plugin(index).unwrap();
            }
            host.build().unwrap();
            let speech_tail = if bypass == Some(0) { 0 } else { 960 };
            let downmix_tail = if bypass == Some(1) {
                0
            } else {
                let accepted = source_frames + speech_tail;
                3072 + (1024 - accepted % 1024) % 1024
            };
            let limiter_tail = if bypass == Some(2) { 0 } else { 240 };
            let latency = DELAY
                - match bypass {
                    Some(0) => 960,
                    Some(1) => 2048,
                    Some(2) => 240,
                    _ => 0,
                };
            assert_eq!(host.total_latency_samples(), latency);
            let mut input: Vec<_> = (0..source_frames * CHANNELS)
                .map(|i| ((i * 7 % 15) as f32 - 7.) / 64.)
                .collect();
            input[(source_frames - 1) * CHANNELS..].copy_from_slice(&[0.25, -0.125]);
            for block in [1, 137, 8193] {
                let output = finite_output(&mut host, &input, CHANNELS, block);
                assert_eq!(
                    output.len(),
                    (source_frames + speech_tail + downmix_tail + limiter_tail) * CHANNELS
                );
                for (index, &actual) in output.iter().enumerate() {
                    let expected = index
                        .checked_sub(latency * CHANNELS)
                        .and_then(|i| input.get(i))
                        .copied()
                        .unwrap_or(0.);
                    assert!(
                        (actual - expected).abs() < 1e-6,
                        "source={source_frames} bypass={bypass:?} block={block} sample={index}: {actual} != {expected}"
                    );
                }
                host.reset();
            }
        }
    }
}
