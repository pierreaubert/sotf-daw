use super::{PluginConfig, build_plugin_host};

fn three_band_both() -> PluginConfig {
    PluginConfig::new(
        "crossover",
        serde_json::json!({
            "type": "LR24",
            "frequency": 420.0,
            "output": "both",
            "topology": "bands",
            "extra_frequencies": [2_200.0],
        }),
    )
}

#[test]
fn crossover_both_width_and_band_merge_match_full_audio_reference() {
    const INPUT_CHANNELS: usize = 2;
    const BANDS: usize = 3;
    const FRAMES: usize = 257;

    let crossover = three_band_both();
    let mut split_host =
        build_plugin_host(std::slice::from_ref(&crossover), 48_000, INPUT_CHANNELS)
            .expect("valid Crossover route must build")
            .0;
    assert_eq!(split_host.output_channels(), INPUT_CHANNELS * BANDS);
    assert!(
        split_host.bypass_plugin(0).is_err(),
        "bypassing a width-changing Crossover must be refused"
    );

    let configs = [
        crossover,
        PluginConfig::new("band_merge", serde_json::json!({ "bands": BANDS })),
    ];
    let mut merged_host = build_plugin_host(&configs, 48_000, INPUT_CHANNELS)
        .expect("Crossover output width must feed BandMerge")
        .0;
    assert_eq!(merged_host.output_channels(), INPUT_CHANNELS);

    let input: Vec<f32> = (0..FRAMES)
        .flat_map(|frame| {
            let time = frame as f32 / 48_000.0;
            [
                (std::f32::consts::TAU * 310.0 * time).sin()
                    + 0.27 * (std::f32::consts::TAU * 3_100.0 * time).sin(),
                0.63 * (std::f32::consts::TAU * 730.0 * time + 0.2).sin()
                    - 0.19 * (std::f32::consts::TAU * 4_200.0 * time).sin(),
            ]
        })
        .collect();
    let mut split_audio = vec![0.0; FRAMES * INPUT_CHANNELS * BANDS];
    let split_frames = split_host.process(&input, &mut split_audio).unwrap();
    assert_eq!(split_frames, FRAMES);
    assert!(split_audio.iter().all(|sample| sample.is_finite()));
    assert!(
        split_audio.iter().any(|sample| sample.abs() > 1.0e-4),
        "the split reference must contain nontrivial audio"
    );

    let mut merged_audio = vec![0.0; FRAMES * INPUT_CHANNELS];
    let merged_frames = merged_host.process(&input, &mut merged_audio).unwrap();
    assert_eq!(merged_frames, FRAMES);
    assert!(merged_audio.iter().all(|sample| sample.is_finite()));
    assert!(
        merged_audio.iter().any(|sample| sample.abs() > 1.0e-4),
        "the composed route must contain nontrivial audio"
    );

    for frame in 0..FRAMES {
        for channel in 0..INPUT_CHANNELS {
            let expected = (0..BANDS)
                .map(|band| {
                    split_audio[frame * INPUT_CHANNELS * BANDS + band * INPUT_CHANNELS + channel]
                })
                .sum::<f32>();
            let actual = merged_audio[frame * INPUT_CHANNELS + channel];
            assert!(
                (actual - expected).abs() <= 2.0e-6,
                "BandMerge differs from the independently processed band sum at frame {frame}, channel {channel}: actual={actual}, expected={expected}"
            );
        }
    }
}

#[test]
fn invalid_crossover_candidate_is_not_silently_skipped() {
    let invalid = PluginConfig::new(
        "crossover",
        serde_json::json!({
            "type": "LR24",
            "frequency": 1_000.0,
            "output": "both",
            "topology": "per_channel",
        }),
    );
    assert!(
        build_plugin_host(&[invalid], 48_000, 2).is_err(),
        "an invalid requested Crossover route must abort candidate construction"
    );
}

#[test]
fn actual_crossover_route_width_matches_each_topology_and_mode() {
    let cases = [
        serde_json::json!({
            "type": "LR24",
            "frequency": 800.0,
            "output": "lowpass",
            "topology": "bands",
        }),
        serde_json::json!({
            "type": "LR24",
            "frequency": 800.0,
            "output": "highpass",
            "topology": "bands",
            "extra_frequencies": [3_000.0],
        }),
    ];
    for parameters in cases {
        let config = PluginConfig::new("crossover", parameters);
        let host = build_plugin_host(&[config], 48_000, 4)
            .expect("valid in-place Crossover route must build")
            .0;
        assert_eq!(host.output_channels(), 4);
    }

    let per_channel = PluginConfig::new(
        "crossover",
        serde_json::json!({
            "type": "LR24",
            "frequency": 800.0,
            "output": "both",
            "topology": "per_channel",
            "extra_frequencies": [3_000.0],
            "channel_frequencies_hz": [500.0, 1_500.0, 2_500.0, 3_500.0],
            "channel_modes": ["lowpass", "highpass", "mute", "passthrough"],
        }),
    );
    let host = build_plugin_host(&[per_channel], 48_000, 4)
        .expect("valid explicit per-channel route must build")
        .0;
    assert_eq!(host.output_channels(), 4);
}
