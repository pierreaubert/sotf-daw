//! Reproducible pre-edit audio capture for AUD145 per-band placement.
//!
//! Run explicitly with `AUD145_BASELINE_DIR=... cargo test -p sotf-plugin-eq
//! --test aud145_legacy_baseline -- --ignored --nocapture`. This fixture uses
//! only the public EQ parameter and processing APIs. Keep this source and its
//! exact artifacts immutable after production routing changes.

use sotf_host::{AutoGainParams, ParameterId, ParameterValue, ParametricPlugin, ProcessContext};
use sotf_plugin_eq::{
    BiquadFilterConfig, EqFilterTopology, EqPlugin, EqPluginParams, KautzSectionConfig,
};
use std::fs;
use std::path::{Path, PathBuf};

const SAMPLE_RATE: u32 = 48_000;
const FRAMES: usize = 8_192;

struct LegacyCase {
    name: &'static str,
    channels: usize,
    params: EqPluginParams,
    oversampling: i32,
    block_size: usize,
    frames: usize,
}

fn config(
    filter_type: &str,
    freq: f64,
    q: f64,
    db_gain: f64,
    topology: EqFilterTopology,
    lambda: Option<f64>,
    kautz_sections: Vec<KautzSectionConfig>,
) -> BiquadFilterConfig {
    BiquadFilterConfig {
        filter_type: filter_type.to_owned(),
        freq,
        q,
        db_gain,
        order: 2,
        topology,
        placement: None,
        lambda,
        kautz_sections,
    }
}

fn input_signal(frames: usize, channels: usize) -> Vec<f32> {
    let mut input = vec![0.0; frames * channels];
    for frame in 0..frames {
        let time = f64::from(frame as u32) / f64::from(SAMPLE_RATE);
        for channel in 0..channels {
            let channel_f = channel as f64;
            let noise = ((frame * 37 + channel * 53) % 251) as f64 / 250.0 - 0.5;
            let mut sample = 0.11
                * (std::f64::consts::TAU * (97.0 + 61.0 * channel_f) * time).sin()
                + 0.07 * (std::f64::consts::TAU * (911.0 + 173.0 * channel_f) * time).cos()
                + 0.015 * noise;
            if frame == 0 {
                sample += 0.2 * (channel_f + 1.0);
            }
            input[frame * channels + channel] = sample as f32;
        }
    }
    input
}

fn cases() -> Vec<LegacyCase> {
    let mixed_order = vec![
        config(
            "peak",
            630.0,
            1.2,
            5.0,
            EqFilterTopology::Biquad,
            None,
            Vec::new(),
        ),
        config(
            "lowpass",
            4_300.0,
            0.8,
            -1.0,
            EqFilterTopology::WarpedBiquad,
            Some(0.25),
            Vec::new(),
        ),
        config(
            "peak",
            8_200.0,
            1.4,
            0.0,
            EqFilterTopology::KautzFilter,
            None,
            vec![KautzSectionConfig {
                pole_freq: 8_200.0,
                q: 1.4,
                gain: 0.04,
            }],
        ),
        config(
            "lowshelf",
            120.0,
            0.8,
            -3.0,
            EqFilterTopology::Biquad,
            None,
            Vec::new(),
        ),
    ];

    let per_channel = vec![
        vec![
            config(
                "peak",
                800.0,
                0.9,
                4.0,
                EqFilterTopology::Biquad,
                None,
                Vec::new(),
            ),
            config(
                "highpass",
                75.0,
                0.7,
                0.0,
                EqFilterTopology::WarpedBiquad,
                None,
                Vec::new(),
            ),
        ],
        vec![
            config(
                "peak",
                1_700.0,
                1.3,
                -5.0,
                EqFilterTopology::Biquad,
                None,
                Vec::new(),
            ),
            config(
                "highshelf",
                6_500.0,
                0.9,
                2.0,
                EqFilterTopology::Biquad,
                None,
                Vec::new(),
            ),
        ],
    ];

    let auto_gain: AutoGainParams = serde_json::from_value(serde_json::json!({
        "enabled": true,
        "max_gain_db": 6.0,
        "smoothing_ms": 80.0
    }))
    .expect("valid AutoGain baseline settings");

    vec![
        LegacyCase {
            name: "mixed_biquad_warped_kautz_stored_order",
            channels: 2,
            params: EqPluginParams {
                filters: mixed_order,
                channel_filters: None,
                stereo_pairs: None,
                auto_gain: AutoGainParams::default(),
            },
            oversampling: 1,
            block_size: 257,
            frames: FRAMES,
        },
        LegacyCase {
            name: "legacy_channel_filters_precedence",
            channels: 2,
            params: EqPluginParams {
                filters: vec![config(
                    "peak",
                    1_000.0,
                    1.0,
                    9.0,
                    EqFilterTopology::Biquad,
                    None,
                    Vec::new(),
                )],
                channel_filters: Some(per_channel),
                stereo_pairs: None,
                auto_gain: AutoGainParams::default(),
            },
            oversampling: 1,
            block_size: 509,
            frames: FRAMES,
        },
        LegacyCase {
            name: "legacy_shared_oversampling_4x",
            channels: 2,
            params: EqPluginParams {
                filters: vec![
                    config(
                        "highpass",
                        42.0,
                        0.707,
                        0.0,
                        EqFilterTopology::Biquad,
                        None,
                        Vec::new(),
                    ),
                    config(
                        "peak",
                        1_150.0,
                        2.0,
                        6.0,
                        EqFilterTopology::Biquad,
                        None,
                        Vec::new(),
                    ),
                    config(
                        "highshelf",
                        9_000.0,
                        0.707,
                        -2.5,
                        EqFilterTopology::Biquad,
                        None,
                        Vec::new(),
                    ),
                ],
                channel_filters: None,
                stereo_pairs: None,
                auto_gain: AutoGainParams::default(),
            },
            oversampling: 4,
            block_size: 383,
            frames: FRAMES,
        },
        LegacyCase {
            name: "legacy_autogain_enabled_whole_plugin",
            channels: 2,
            params: EqPluginParams {
                filters: vec![config(
                    "peak",
                    1_000.0,
                    1.0,
                    9.0,
                    EqFilterTopology::Biquad,
                    None,
                    Vec::new(),
                )],
                channel_filters: None,
                stereo_pairs: None,
                auto_gain,
            },
            oversampling: 1,
            block_size: 256,
            frames: 96_000,
        },
    ]
}

fn capture_case(case: &LegacyCase, directory: &Path) {
    let case_dir = directory.join(case.name);
    fs::create_dir_all(&case_dir).expect("create case artifact directory");

    let input = input_signal(case.frames, case.channels);
    let mut plugin = EqPlugin::from_params(case.channels, SAMPLE_RATE, case.params.clone())
        .expect("construct legacy EQ fixture");
    plugin
        .parametric_set_parameter(
            ParameterId::from("oversampling"),
            ParameterValue::Int(case.oversampling),
        )
        .expect("set legacy oversampling control before initialization");
    assert_eq!(
        plugin.parametric_get_parameter(&ParameterId::from("oversampling")),
        Some(ParameterValue::Int(case.oversampling)),
        "requested oversampling setting was not active"
    );
    plugin
        .plugin_initialize(SAMPLE_RATE)
        .expect("initialize EQ");

    let disabled_autogain_output = if case.name == "legacy_autogain_enabled_whole_plugin" {
        let mut disabled_params = case.params.clone();
        disabled_params.auto_gain.enabled = false;
        let mut disabled = EqPlugin::from_params(case.channels, SAMPLE_RATE, disabled_params)
            .expect("construct AutoGain-disabled comparison EQ");
        disabled
            .plugin_initialize(SAMPLE_RATE)
            .expect("initialize disabled AutoGain comparison EQ");
        Some(render_blocks(
            &mut disabled,
            &input,
            case.channels,
            case.block_size,
        ))
    } else {
        None
    };
    let output = render_blocks(&mut plugin, &input, case.channels, case.block_size);

    assert_eq!(input.len(), output.len());
    assert!(output.iter().all(|sample| sample.is_finite()));
    assert!(output.iter().any(|sample| sample.abs() > 1e-5));
    if let Some(disabled) = disabled_autogain_output {
        let rms_difference = output_rms_difference(&output, &disabled);
        assert!(
            rms_difference > 1e-4,
            "AutoGain did not change the fully measured render: {rms_difference}"
        );
        fs::write(
            case_dir.join("output_autogain_disabled.f32le"),
            encode_f32(&disabled),
        )
        .expect("write AutoGain-disabled twin");
        fs::write(
            case_dir.join("autogain_comparison.json"),
            serde_json::to_vec_pretty(&serde_json::json!({
                "rms_difference_enabled_vs_disabled": rms_difference,
                "frames": case.frames,
                "seconds": case.frames as f64 / f64::from(SAMPLE_RATE),
                "measurement_type": "momentary",
                "smoothing_ms": case.params.auto_gain.smoothing_ms,
            }))
            .unwrap(),
        )
        .expect("write AutoGain comparison metrics");
    }

    fs::write(case_dir.join("input.f32le"), encode_f32(&input)).expect("write exact input vector");
    fs::write(case_dir.join("output.f32le"), encode_f32(&output))
        .expect("write exact legacy output vector");
    fs::write(
        case_dir.join("settings.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "sample_rate": SAMPLE_RATE,
            "channels": case.channels,
            "frames": case.frames,
            "block_size": case.block_size,
            "oversampling": case.oversampling,
            "oversampling_readback": plugin
                .parametric_get_parameter(&ParameterId::from("oversampling")),
            "params": case.params,
        }))
        .expect("serialize exact public EQ settings"),
    )
    .expect("write settings");
}

fn render_blocks(
    plugin: &mut EqPlugin,
    input: &[f32],
    channels: usize,
    block_size: usize,
) -> Vec<f32> {
    let mut output = vec![0.0; input.len()];
    for (input_block, output_block) in input
        .chunks(block_size * channels)
        .zip(output.chunks_mut(block_size * channels))
    {
        let frames = input_block.len() / channels;
        let context = ProcessContext::new(SAMPLE_RATE, frames);
        let processed = plugin
            .process(input_block, output_block, &context)
            .expect("process public legacy EQ route");
        assert_eq!(processed, frames, "short public EQ output");
    }
    output
}

fn output_rms_difference(left: &[f32], right: &[f32]) -> f64 {
    assert_eq!(left.len(), right.len());
    let sum_squares = left
        .iter()
        .zip(right)
        .map(|(&a, &b)| f64::from(a - b).powi(2))
        .sum::<f64>();
    (sum_squares / left.len() as f64).sqrt()
}

fn encode_f32(samples: &[f32]) -> Vec<u8> {
    let mut encoded = Vec::with_capacity(std::mem::size_of_val(samples));
    for sample in samples {
        encoded.extend_from_slice(&sample.to_le_bytes());
    }
    encoded
}

fn render_public_params(params: EqPluginParams) -> Vec<f32> {
    let channels = 2;
    let input = input_signal(FRAMES, channels);
    let mut plugin = EqPlugin::from_params(channels, SAMPLE_RATE, params).unwrap();
    plugin.plugin_initialize(SAMPLE_RATE).unwrap();
    let block_size = 256;
    render_blocks(&mut plugin, &input, channels, block_size)
}

#[test]
#[ignore = "explicit pre-edit artifact capture; set AUD145_BASELINE_DIR"]
fn capture_pre_edit_legacy_audio_and_public_placement_gap() {
    let output_dir = PathBuf::from(
        std::env::var_os("AUD145_BASELINE_DIR")
            .expect("set AUD145_BASELINE_DIR to the durable artifact directory"),
    );
    fs::create_dir_all(&output_dir).expect("create AUD145 artifact root");

    let mut case_names = Vec::new();
    for case in cases() {
        case_names.push(case.name);
        capture_case(&case, &output_dir);
    }

    let plain_json = serde_json::json!({
        "filters": [{
            "filter_type": "peak",
            "freq": 1000.0,
            "q": 1.0,
            "db_gain": 6.0
        }],
        "auto_gain": { "enabled": false }
    });
    let placement_json = serde_json::json!({
        "filters": [{
            "filter_type": "peak",
            "freq": 1000.0,
            "q": 1.0,
            "db_gain": 6.0,
            "placement": "mid"
        }],
        "auto_gain": { "enabled": false }
    });
    let plain: EqPluginParams = serde_json::from_value(plain_json).unwrap();
    let requested: EqPluginParams = serde_json::from_value(placement_json).unwrap();
    let normalized_plain = serde_json::to_value(&plain).unwrap();
    let normalized_requested = serde_json::to_value(&requested).unwrap();
    assert_eq!(normalized_requested, normalized_plain);
    assert_eq!(
        render_public_params(plain),
        render_public_params(requested),
        "unrecognized public placement must be shown to have no DSP effect"
    );

    fs::write(
        output_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&serde_json::json!({
            "status": "pre-edit public legacy capture",
            "sample_rate": SAMPLE_RATE,
            "default_frames_per_case": FRAMES,
            "cases": case_names,
            "public_gap": {
                "request": "placement=mid on a serialized band",
                "observation": "serde accepts and drops the unknown field; normalized settings and output are identical to absent placement"
            }
        }))
        .unwrap(),
    )
    .expect("write baseline capture index");
}
