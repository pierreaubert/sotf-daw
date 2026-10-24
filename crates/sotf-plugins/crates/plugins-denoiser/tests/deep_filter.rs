//! Actual DeepFilterNet2 preparation, pinned-runtime consistency, and pristine reset.
#![cfg(feature = "deep-filter")]

use deep_filter_runtime::tract::{DfParams, DfTract, RuntimeParams};
use ndarray015::{ArrayView2, ArrayViewMut2};
use plugins_denoiser::deep_filter::{DEEP_FILTER_HOP, DeepFilterModel};
use plugins_denoiser::deep_filter_config::DeepFilterConfig;
use plugins_inference::framed_audio::AudioFrameModel;
use std::path::PathBuf;

fn config() -> DeepFilterConfig {
    DeepFilterConfig {
        model_path: PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../models/deepfilternet2/DeepFilterNet2_onnx.tar.gz")
            .to_string_lossy()
            .into_owned(),
        sha256: "412e81d8d1779b8f925ac4b53472655a9f866c4af73dbf930ab587359f631628".into(),
        playout_packets: 4,
    }
}

fn input(packet: usize) -> [f32; DEEP_FILTER_HOP] {
    std::array::from_fn(|i| {
        let n = (packet * DEEP_FILTER_HOP + i) as f64;
        (0.2 * (std::f64::consts::TAU * 193.0 * n / 48_000.0).sin()
            + 0.08 * (std::f64::consts::TAU * 386.0 * n / 48_000.0).sin()
            + 0.025 * (std::f64::consts::TAU * 7171.0 * n / 48_000.0).sin()) as f32
    })
}

#[test]
fn verified_wrapper_matches_the_pinned_runtime() {
    let config = config();
    let mut wrapper = DeepFilterModel::load(&config).unwrap();
    let mut reference = DfTract::new(
        DfParams::new(config.model_path.into()).unwrap(),
        &RuntimeParams::default_with_ch(1),
    )
    .unwrap();
    assert_eq!(wrapper.latency_samples(), 2400);
    let mut max_error = 0.0_f32;
    for packet in 0..40 {
        let source = input(packet);
        let mut actual = [0.0; DEEP_FILTER_HOP];
        let mut expected = actual;
        let actual_score = wrapper.process(&source, &mut actual).unwrap();
        let expected_score = reference
            .process(
                ArrayView2::from_shape((1, DEEP_FILTER_HOP), &source).unwrap(),
                ArrayViewMut2::from_shape((1, DEEP_FILTER_HOP), &mut expected).unwrap(),
            )
            .unwrap();
        for (a, b) in actual.iter().zip(expected) {
            max_error = max_error.max((a - b).abs());
        }
        assert!((actual_score - expected_score).abs() < 1e-5);
    }
    println!("DeepFilterNet2 pinned-runtime maximum waveform error {max_error}");
    assert!(max_error < 1e-6);
}

#[test]
fn pristine_reset_restores_recurrent_normalization_and_stft_state() {
    let mut tested = DeepFilterModel::load(&config()).unwrap();
    let mut fresh = DeepFilterModel::load(&config()).unwrap();
    for packet in 0..30 {
        tested
            .process(&input(packet), &mut [0.0; DEEP_FILTER_HOP])
            .unwrap();
    }
    tested.reset().unwrap();
    let mut max_error = 0.0_f32;
    for packet in 0..30 {
        let mut actual = [0.0; DEEP_FILTER_HOP];
        let mut expected = actual;
        tested.process(&input(packet + 300), &mut actual).unwrap();
        fresh.process(&input(packet + 300), &mut expected).unwrap();
        for (a, b) in actual.iter().zip(expected) {
            max_error = max_error.max((a - b).abs());
        }
    }
    println!("DeepFilterNet2 pristine-reset maximum waveform error {max_error}");
    assert!(max_error < 1e-6);
}

#[test]
fn hash_and_playout_fail_before_model_adoption() {
    let mut bad = config();
    bad.sha256 = "0".repeat(64);
    assert!(DeepFilterModel::load(&bad).is_err());
    let mut bad = config();
    bad.playout_packets = 100;
    assert!(DeepFilterModel::load(&bad).is_err());
}

#[test]
fn independently_measured_unity_mask_delay_matches_reported_latency() {
    for channels in [1, 2] {
        let mut reference = DfTract::new(
            DfParams::new(config().model_path.into()).unwrap(),
            &RuntimeParams::default_with_ch(channels),
        )
        .unwrap();
        // Preserve both STFT histories and rolling spectra while bypassing
        // learned masks and coefficients. This independently measures delay.
        reference.min_db_thresh = -1e6;
        reference.max_db_erb_thresh = -1e6;
        reference.max_db_df_thresh = -1e6;
        let delay = 2400;
        let mut source = Vec::new();
        let mut rendered = Vec::new();
        for packet in 0..50 {
            let left = input(packet);
            let right = input(packet + 43);
            let mut planar = vec![0.; channels * DEEP_FILTER_HOP];
            planar[..DEEP_FILTER_HOP].copy_from_slice(&left);
            if channels == 2 {
                planar[DEEP_FILTER_HOP..].copy_from_slice(&right);
            }
            let mut output = vec![0.; planar.len()];
            reference
                .process(
                    ArrayView2::from_shape((channels, DEEP_FILTER_HOP), &planar).unwrap(),
                    ArrayViewMut2::from_shape((channels, DEEP_FILTER_HOP), &mut output).unwrap(),
                )
                .unwrap();
            for frame in 0..DEEP_FILTER_HOP {
                for channel in 0..channels {
                    source.push(planar[channel * DEEP_FILTER_HOP + frame]);
                    rendered.push(output[channel * DEEP_FILTER_HOP + frame]);
                }
            }
        }
        let max_error = rendered
            .iter()
            .enumerate()
            .map(|(i, &actual)| {
                let expected = if i < delay * channels {
                    0.
                } else {
                    source[i - delay * channels]
                };
                (actual - expected).abs()
            })
            .fold(0f32, f32::max);
        println!(
            "DeepFilterNet2 unity-mask channels={channels} delay={delay}, maximum waveform error {max_error}"
        );
        assert!(
            max_error < 1e-6,
            "reported delay does not match source audio: {max_error}"
        );
    }
}

#[test]
fn linked_two_channel_runtime_preserves_fixed_channel_ratio() {
    for (ratio, post_filter) in [
        (0.25_f32, false),
        (-0.25, false),
        (0.0, false),
        (0.25, true),
    ] {
        let mut model = DfTract::new(
            DfParams::new(config().model_path.into()).unwrap(),
            &RuntimeParams::new(
                2,
                post_filter,
                100.,
                -1e6,
                1e6,
                1e6,
                deep_filter_runtime::tract::ReduceMask::MEAN,
            ),
        )
        .unwrap();
        // Exercise both learned stages rather than passing on a dry-only path.
        model.min_db_thresh = -1e6;
        model.max_db_erb_thresh = 1e6;
        model.max_db_df_thresh = 1e6;
        let mut maximum_error = 0f32;
        let mut energy = 0f64;
        for packet in 0..40 {
            let left = input(packet);
            let mut source = [0.; DEEP_FILTER_HOP * 2];
            source[..DEEP_FILTER_HOP].copy_from_slice(&left);
            for (dst, &sample) in source[DEEP_FILTER_HOP..].iter_mut().zip(&left) {
                *dst = sample * ratio;
            }
            let mut output = [0.; DEEP_FILTER_HOP * 2];
            model
                .process(
                    ArrayView2::from_shape((2, DEEP_FILTER_HOP), &source).unwrap(),
                    ArrayViewMut2::from_shape((2, DEEP_FILTER_HOP), &mut output).unwrap(),
                )
                .unwrap();
            for (&left, &right) in output[..DEEP_FILTER_HOP]
                .iter()
                .zip(&output[DEEP_FILTER_HOP..])
            {
                maximum_error = maximum_error.max((right - ratio * left).abs());
                energy += f64::from(left).powi(2);
            }
        }
        println!("Linked DFN2 ratio={ratio}: max error={maximum_error}, energy={energy}");
        assert!(energy > 1e-6, "the learned stages must produce audio");
        assert!(maximum_error < 1e-6, "linked transfer functions diverged");
    }
}

#[test]
fn linked_adapter_is_channel_swap_equivariant_and_restores_both_histories() {
    use plugins_denoiser::deep_filter::LinkedDeepFilterModel;
    let mut tested = LinkedDeepFilterModel::load(&config()).unwrap();
    let mut swapped = LinkedDeepFilterModel::load(&config()).unwrap();
    let mut maximum = 0f32;
    for packet in 0..40 {
        let left = input(packet);
        let right = input(packet + 43);
        let source: [f32; DEEP_FILTER_HOP * 2] = std::array::from_fn(|i| {
            if i % 2 == 0 {
                left[i / 2]
            } else {
                0.17 * right[i / 2]
            }
        });
        let reverse = std::array::from_fn(|i| source[i ^ 1]);
        let mut actual = [0.; DEEP_FILTER_HOP * 2];
        let mut reversed = actual;
        let score = tested.process(&source, &mut actual).unwrap();
        let swapped_score = swapped.process(&reverse, &mut reversed).unwrap();
        assert!((score - swapped_score).abs() < 1e-5);
        for (i, &value) in actual.iter().enumerate() {
            maximum = maximum.max((value - reversed[i ^ 1]).abs());
        }
    }
    assert!(
        maximum < 1e-6,
        "channel order changed the shared decision: {maximum}"
    );
    tested.reset().unwrap();
    let mut fresh = LinkedDeepFilterModel::load(&config()).unwrap();
    let mut reset_error = 0f32;
    for packet in 0..40 {
        let source =
            std::array::from_fn(|i| signal_for_reset(packet * DEEP_FILTER_HOP + i / 2, i % 2));
        let mut actual = [0.; DEEP_FILTER_HOP * 2];
        let mut expected = actual;
        tested.process(&source, &mut actual).unwrap();
        fresh.process(&source, &mut expected).unwrap();
        for (&a, b) in actual.iter().zip(expected) {
            reset_error = reset_error.max((a - b).abs());
        }
    }
    println!("Linked adapter swap error={maximum}, reset error={reset_error}");
    assert!(reset_error < 1e-6);
}

fn signal_for_reset(n: usize, channel: usize) -> f32 {
    let frequency = if channel == 0 { 347.0 } else { 1379.0 };
    (0.2 * (std::f64::consts::TAU * frequency * n as f64 / 48_000.0).sin()) as f32
}

#[test]
fn mono_port_matches_unchanged_upstream_v031_golden_audio() {
    use sha2::{Digest, Sha256};
    let hash = |bytes: &[u8]| -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    };
    let input = &include_bytes!("data/deepfilter-v031/input.f32le")[..];
    let waveform = &include_bytes!("data/deepfilter-v031/waveform.f32le")[..];
    let scores = &include_bytes!("data/deepfilter-v031/scores.f32le")[..];
    assert_eq!(
        hash(input),
        "9107f19e1a6d3ad50d5b0e19f120fa1b078ab3388245dee7c12bc38f9cdb1f00"
    );
    assert_eq!(
        hash(waveform),
        "8021ca05dd6a5206d9cda1aafc54e7b3810106bdacfeae37e3d0c1f6db5af58a"
    );
    assert_eq!(
        hash(scores),
        "f1b0e558150fbf0794d12ca4cb6ba1d2cef2ede232e86bcbcf44360d3b136351"
    );
    let decode = |bytes: &[u8]| -> Vec<f32> {
        assert!(bytes.len().is_multiple_of(4));
        bytes
            .as_chunks::<4>()
            .0
            .iter()
            .map(|frame| f32::from_le_bytes(*frame))
            .collect()
    };
    let input = decode(input);
    let waveform = decode(waveform);
    let scores = decode(scores);
    assert_eq!(input.len(), DEEP_FILTER_HOP * 64);
    assert_eq!(waveform.len(), input.len());
    assert_eq!(scores.len(), 64);
    assert!(waveform.iter().any(|sample| sample.abs() > 1e-6));
    let mut model = DeepFilterModel::load(&config()).unwrap();
    let mut max_waveform_error = 0f32;
    let mut max_score_error = 0f32;
    for (packet, source) in input.as_chunks::<DEEP_FILTER_HOP>().0.iter().enumerate() {
        let mut actual = [0.; DEEP_FILTER_HOP];
        let score = model.process(source, &mut actual).unwrap();
        max_score_error = max_score_error.max((score - scores[packet]).abs());
        for (i, sample) in actual.iter().enumerate() {
            max_waveform_error =
                max_waveform_error.max((sample - waveform[packet * DEEP_FILTER_HOP + i]).abs());
        }
    }
    println!(
        "Unchanged upstream DFN2 golden: waveform_error={max_waveform_error}, score_error={max_score_error}"
    );
    assert!(max_waveform_error < 1e-6);
    assert!(max_score_error < 1e-5);
}
