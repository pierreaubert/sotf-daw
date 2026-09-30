//! Black-box integration tests for `sotf-plugin-ambisonics`.
//!
//! These tests exercise the public `Plugin` API surface from outside the crate.

// Rust guideline compliant 2026-02-21

use sotf_host::parameters::{ParameterId, ParameterValue};
use sotf_host::plugin::{Plugin, ProcessContext, TailLength};
use sotf_host::speaker_config::get_speaker_config;
use sotf_plugin_ambisonics::decode_matrix::DecodeMatrix;
use sotf_plugin_ambisonics::{AmbisonicsDecoderConfig, AmbisonicsDecoderPlugin};

fn foa_5_1_config() -> AmbisonicsDecoderConfig {
    AmbisonicsDecoderConfig {
        order: 1,
        target_layout: "5.1".to_owned(),
        max_re_weighting: true,
        dual_band: false,
        algorithm: "mode_matching".to_owned(),
    }
}

#[test]
fn construct_foa_5_1() {
    let plugin = AmbisonicsDecoderPlugin::new(&foa_5_1_config()).unwrap();
    assert_eq!(plugin.input_channels(), 4);
    assert_eq!(plugin.output_channels(), 6);
    assert_eq!(plugin.info().name, "AmbisonicsDecoder");
}

#[test]
fn construct_soa_7_1_4() {
    let config = AmbisonicsDecoderConfig {
        order: 2,
        target_layout: "7.1.4".to_owned(),
        max_re_weighting: false,
        dual_band: false,
        algorithm: "mode_matching".to_owned(),
    };
    let plugin = AmbisonicsDecoderPlugin::new(&config).unwrap();
    assert_eq!(plugin.input_channels(), 9);
    assert_eq!(plugin.output_channels(), 12);
}

#[test]
fn construct_allrad_mode_from_serialized_config() {
    let config = AmbisonicsDecoderConfig {
        algorithm: "allrad".to_owned(),
        ..foa_5_1_config()
    };
    let plugin = AmbisonicsDecoderPlugin::new(&config).unwrap();
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("algorithm")),
        Some(ParameterValue::Int(1))
    );
}

#[test]
fn invalid_layout_returns_error() {
    let config = AmbisonicsDecoderConfig {
        order: 1,
        target_layout: "not-a-layout".to_owned(),
        max_re_weighting: true,
        dual_band: false,
        algorithm: "mode_matching".to_owned(),
    };
    assert!(AmbisonicsDecoderPlugin::new(&config).is_err());
}

#[test]
fn parameters_listed_by_trait() {
    let plugin = AmbisonicsDecoderPlugin::new(&foa_5_1_config()).unwrap();
    let params = plugin.parameters();
    let ids: Vec<_> = params.iter().map(|p| p.id.as_str()).collect();
    assert!(ids.contains(&"order"));
    assert!(ids.contains(&"target_layout"));
    assert!(ids.contains(&"max_re_weighting"));
    assert!(ids.contains(&"dual_band"));
}

#[test]
fn structural_parameter_changes_are_rejected() {
    let mut plugin = AmbisonicsDecoderPlugin::new(&foa_5_1_config()).unwrap();

    assert_eq!(
        plugin.get_parameter(&ParameterId::from("order")),
        Some(ParameterValue::Int(1))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("max_re_weighting")),
        Some(ParameterValue::Bool(true))
    );
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("dual_band")),
        Some(ParameterValue::Bool(false))
    );

    let error = plugin
        .set_parameter(
            ParameterId::from("max_re_weighting"),
            ParameterValue::Bool(false),
        )
        .unwrap_err();
    assert!(error.contains("host rebuild"));
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("max_re_weighting")),
        Some(ParameterValue::Bool(true))
    );
}

#[test]
fn order_change_requires_host_rebuild() {
    // Use a layout with enough speakers for second-order ambisonics.
    let config = AmbisonicsDecoderConfig {
        order: 1,
        target_layout: "7.1.4".to_owned(),
        max_re_weighting: true,
        dual_band: false,
        algorithm: "mode_matching".to_owned(),
    };
    let mut plugin = AmbisonicsDecoderPlugin::new(&config).unwrap();
    plugin.initialize(48000).unwrap();

    let error = plugin
        .set_parameter(ParameterId::from("order"), ParameterValue::Int(2))
        .unwrap_err();
    assert!(error.contains("host rebuild"));
    assert_eq!(plugin.input_channels(), 4);
    assert_eq!(plugin.output_channels(), 12);
}

#[test]
fn layout_choice_change_requires_host_rebuild() {
    let mut plugin = AmbisonicsDecoderPlugin::new(&foa_5_1_config()).unwrap();
    plugin.initialize(48000).unwrap();

    assert_eq!(
        plugin.get_parameter(&ParameterId::from("target_layout")),
        Some(ParameterValue::Int(0))
    );
    let error = plugin
        .set_parameter(ParameterId::from("target_layout"), ParameterValue::Int(1))
        .unwrap_err();
    assert!(error.contains("host rebuild"));
    assert_eq!(plugin.output_channels(), 6);
}

#[test]
fn invalid_layout_parameter_rejected() {
    let mut plugin = AmbisonicsDecoderPlugin::new(&foa_5_1_config()).unwrap();
    let result = plugin.set_parameter(ParameterId::from("target_layout"), ParameterValue::Int(999));
    assert!(result.is_err());
}

#[test]
fn process_silence_produces_silence() {
    let mut plugin = AmbisonicsDecoderPlugin::new(&foa_5_1_config()).unwrap();
    plugin.initialize(48000).unwrap();

    let num_frames = 256;
    let input = vec![0.0_f32; num_frames * 4];
    let mut output = vec![0.0_f32; num_frames * 6];
    let ctx = ProcessContext::new(48000, num_frames);

    let frames = plugin.process(&input, &mut output, &ctx).unwrap();
    assert_eq!(frames, num_frames);
    assert!(output.iter().all(|s| s.abs() < 1e-9));
}

#[test]
fn process_omni_signal_reaches_all_speakers() {
    let mut plugin = AmbisonicsDecoderPlugin::new(&foa_5_1_config()).unwrap();
    plugin.initialize(48000).unwrap();

    let num_frames = 64;
    // Pure W (omnidirectional) signal
    let mut input = vec![0.0_f32; num_frames * 4];
    for frame in 0..num_frames {
        input[frame * 4] = 0.5;
    }
    let mut output = vec![0.0_f32; num_frames * 6];
    let ctx = ProcessContext::new(48000, num_frames);

    plugin.process(&input, &mut output, &ctx).unwrap();

    // Non-LFE channels should be non-zero
    let non_lfe: Vec<f32> = output.iter().skip(1).step_by(6).copied().collect();
    assert!(non_lfe.iter().any(|s| s.abs() > 1e-6));
}

#[test]
fn dual_band_toggle_via_parameter() {
    let mut plugin = AmbisonicsDecoderPlugin::new(&foa_5_1_config()).unwrap();
    plugin.initialize(48000).unwrap();

    assert_eq!(
        plugin.get_parameter(&ParameterId::from("dual_band")),
        Some(ParameterValue::Bool(false))
    );

    let error = plugin
        .set_parameter(ParameterId::from("dual_band"), ParameterValue::Bool(true))
        .unwrap_err();
    assert!(error.contains("host rebuild"));
    assert_eq!(
        plugin.get_parameter(&ParameterId::from("dual_band")),
        Some(ParameterValue::Bool(false))
    );
}

#[test]
fn buffer_size_mismatch_is_error() {
    let mut plugin = AmbisonicsDecoderPlugin::new(&foa_5_1_config()).unwrap();
    plugin.initialize(48000).unwrap();

    let ctx = ProcessContext::new(48000, 32);
    let input = vec![0.0_f32; 32 * 4 - 1];
    let mut output = vec![0.0_f32; 32 * 6];
    assert!(plugin.process(&input, &mut output, &ctx).is_err());

    let input_ok = vec![0.0_f32; 32 * 4];
    let mut output_short = vec![0.0_f32; 32 * 6 - 1];
    assert!(plugin.process(&input_ok, &mut output_short, &ctx).is_err());
}

#[test]
fn reset_then_process_again() {
    let mut plugin = AmbisonicsDecoderPlugin::new(&foa_5_1_config()).unwrap();
    plugin.initialize(48000).unwrap();

    let num_frames = 256;
    let input = vec![0.1_f32; num_frames * 4];
    let mut output = vec![0.0_f32; num_frames * 6];
    let ctx = ProcessContext::new(48000, num_frames);
    plugin.process(&input, &mut output, &ctx).unwrap();

    plugin.reset();

    let mut output2 = vec![0.0_f32; num_frames * 6];
    plugin.process(&input, &mut output2, &ctx).unwrap();
    assert!(output2.iter().all(|s| s.is_finite()));
}

#[test]
fn serialized_orders_one_through_seven_keep_structural_parameter_contract() {
    for order in 1..=7 {
        let config = AmbisonicsDecoderConfig {
            order,
            target_layout: "9.1.6".to_owned(),
            max_re_weighting: false,
            dual_band: false,
            algorithm: "mode_matching".to_owned(),
        };
        let mut plugin = AmbisonicsDecoderPlugin::new(&config).unwrap();
        assert_eq!(plugin.input_channels(), (order + 1) * (order + 1));
        assert_eq!(plugin.output_channels(), 16);
        assert!(plugin.supports_channel_config(plugin.input_channels(), 16));
        assert_eq!(
            plugin.get_parameter(&ParameterId::from("order")),
            Some(ParameterValue::Int(order as i32))
        );
        let order_parameter = plugin
            .parameters()
            .into_iter()
            .find(|parameter| parameter.id.as_str() == "order")
            .unwrap();
        assert_eq!(order_parameter.min_value, Some(ParameterValue::Int(1)));
        assert_eq!(order_parameter.max_value, Some(ParameterValue::Int(7)));

        if order < 7 {
            let error = plugin
                .set_parameter(ParameterId::from("order"), ParameterValue::Int(7))
                .unwrap_err();
            assert!(error.contains("host rebuild"));
            assert_eq!(plugin.input_channels(), (order + 1) * (order + 1));
        }
    }

    let legacy: AmbisonicsDecoderConfig = serde_json::from_value(serde_json::json!({
        "order": 3,
        "target_layout": "9.1.6",
        "max_re_weighting": true,
        "dual_band": false,
        "algorithm": "mode_matching"
    }))
    .unwrap();
    let legacy_plugin = AmbisonicsDecoderPlugin::new(&legacy).unwrap();
    assert_eq!(legacy_plugin.input_channels(), 16);
    assert_eq!(legacy_plugin.output_channels(), 16);
}

#[test]
fn order_seven_basis_impulses_match_each_public_decoder_matrix_column() {
    let speaker_config = get_speaker_config("9.1.6").unwrap();
    for algorithm in ["mode_matching", "allrad"] {
        let config = AmbisonicsDecoderConfig {
            order: 7,
            target_layout: "9.1.6".to_owned(),
            max_re_weighting: false,
            dual_band: false,
            algorithm: algorithm.to_owned(),
        };
        let mut plugin = AmbisonicsDecoderPlugin::new(&config).unwrap();
        plugin.initialize(48_000).unwrap();
        let expected = if algorithm == "allrad" {
            DecodeMatrix::build_allrad(7, speaker_config, false).unwrap()
        } else {
            DecodeMatrix::build(7, speaker_config, false).unwrap()
        };
        assert_eq!(plugin.input_channels(), 64);
        assert_eq!(plugin.output_channels(), 16);
        for acn in 0..64 {
            let mut input = [0.0_f32; 64];
            input[acn] = 1.0;
            let mut output = [0.0_f32; 16];
            let frames = plugin
                .process(&input, &mut output, &ProcessContext::new(48_000, 1))
                .unwrap();
            assert_eq!(frames, 1);
            assert!(output.iter().all(|sample| sample.is_finite()));
            for (speaker, actual) in output.iter().enumerate() {
                let expected = expected.matrix[speaker * 64 + acn];
                assert!(
                    (*actual - expected).abs() <= 1.0e-6,
                    "algorithm={algorithm}, speaker={speaker}, ACN={acn}: actual={actual}, expected={expected}"
                );
            }
        }
        assert!(
            (0..expected.speaker_count)
                .any(|speaker| expected.matrix[speaker * 64 + 63].abs() > 1.0e-9),
            "algorithm={algorithm} did not route ACN63"
        );

        let mut dense = vec![0.0_f32; 37 * 64];
        for (index, sample) in dense.iter_mut().enumerate() {
            *sample = ((index * 73 % 509) as f32 - 254.0) / 4096.0;
        }
        let mut dense_output = vec![0.0_f32; 37 * 16];
        assert_eq!(
            plugin
                .process(&dense, &mut dense_output, &ProcessContext::new(48_000, 37),)
                .unwrap(),
            37
        );
        assert!(dense_output.iter().all(|sample| sample.is_finite()));

        let mut invalid = vec![0.0_f32; 64];
        invalid[63] = f32::NAN;
        let mut sentinel = [3.25_f32; 16];
        assert!(plugin
            .process(&invalid, &mut sentinel, &ProcessContext::new(48_000, 1),)
            .is_err());
        assert_eq!(sentinel, [3.25; 16]);
    }
}

#[test]
fn order_seven_dual_band_processes_channel_sixty_three_and_keeps_eos_contract() {
    for algorithm in ["mode_matching", "allrad"] {
        let config = AmbisonicsDecoderConfig {
            order: 7,
            target_layout: "9.1.6".to_owned(),
            max_re_weighting: true,
            dual_band: true,
            algorithm: algorithm.to_owned(),
        };
        let mut plugin = AmbisonicsDecoderPlugin::new(&config).unwrap();
        plugin.initialize(48_000).unwrap();
        assert_eq!(plugin.input_channels(), 64);
        assert_eq!(plugin.tail_length(), TailLength::Unknown);
        assert_eq!(plugin.drain_call_bound().unwrap().get(), 1);

        let frames = 3_072;
        let mut input = vec![0.0_f32; frames * 64];
        input[63] = 0.5;
        let mut output = vec![0.0_f32; frames * 16];
        let partitions = [1, 17, 137, 256, 613, 1_024];
        let mut position = 0;
        let mut partition_index = 0;
        while position < frames {
            let block_frames =
                partitions[partition_index % partitions.len()].min(frames - position);
            let input_start = position * 64;
            let output_start = position * 16;
            assert_eq!(
                plugin
                    .process(
                        &input[input_start..input_start + block_frames * 64],
                        &mut output[output_start..output_start + block_frames * 16],
                        &ProcessContext::new(48_000, block_frames)
                            .with_sample_position(position as u64),
                    )
                    .unwrap(),
                block_frames
            );
            position += block_frames;
            partition_index += 1;
        }
        assert!(output.iter().all(|sample| sample.is_finite()));
        assert!(
            output.iter().any(|sample| sample.abs() > 1.0e-9),
            "algorithm={algorithm} failed to process ACN 63 through the crossover"
        );

        plugin.reset();
        let mut replay = vec![0.0_f32; frames * 16];
        let mut position = 0;
        while position < frames {
            let block_frames = partitions[position % partitions.len()].min(frames - position);
            let input_start = position * 64;
            let output_start = position * 16;
            plugin
                .process(
                    &input[input_start..input_start + block_frames * 64],
                    &mut replay[output_start..output_start + block_frames * 16],
                    &ProcessContext::new(48_000, block_frames)
                        .with_sample_position(position as u64),
                )
                .unwrap();
            position += block_frames;
        }
        assert_eq!(output, replay);

        let mut sentinel = [6.5_f32; 4];
        let drain = plugin
            .drain(&mut sentinel, &ProcessContext::new(48_000, 0))
            .unwrap();
        assert_eq!(drain.frames, 0);
        assert!(drain.complete);
        assert_eq!(sentinel, [6.5; 4]);
    }
}

#[test]
fn latency_is_zero() {
    let plugin = AmbisonicsDecoderPlugin::new(&foa_5_1_config()).unwrap();
    assert_eq!(plugin.latency_samples(), 0);
}

#[test]
fn channel_config_support() {
    let plugin = AmbisonicsDecoderPlugin::new(&foa_5_1_config()).unwrap();
    assert!(plugin.supports_channel_config(4, 6));
    assert!(!plugin.supports_channel_config(2, 6));
    assert!(!plugin.supports_channel_config(4, 8));
}

#[test]
fn output_rate_and_frame_mapping() {
    let plugin = AmbisonicsDecoderPlugin::new(&foa_5_1_config()).unwrap();
    assert_eq!(plugin.output_sample_rate(96000), 96000);
    assert_eq!(plugin.output_frames_for_input(128), 128);
}

#[test]
fn invalid_layout_parameter_rejected_by_set() {
    let mut plugin = AmbisonicsDecoderPlugin::new(&foa_5_1_config()).unwrap();
    let result = plugin.set_parameter(ParameterId::from("target_layout"), ParameterValue::Int(999));
    assert!(result.is_err());
}
