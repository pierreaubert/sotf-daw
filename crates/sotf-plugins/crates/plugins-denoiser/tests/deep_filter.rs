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
    let mut reference = DfTract::new(
        DfParams::new(config().model_path.into()).unwrap(),
        &RuntimeParams::default_with_ch(1),
    )
    .unwrap();
    // Force the upstream clean-speech branch: preserve the STFT and rolling
    // spectra while applying neither the learned mask nor deep coefficients.
    reference.min_db_thresh = -1e6;
    reference.max_db_erb_thresh = -1e6;
    reference.max_db_df_thresh = -1e6;
    let delay = 2400;
    let mut source = Vec::new();
    let mut rendered = Vec::new();
    for packet in 0..50 {
        let frame = input(packet);
        source.extend_from_slice(&frame);
        let mut output = [0.; DEEP_FILTER_HOP];
        reference
            .process(
                ArrayView2::from_shape((1, DEEP_FILTER_HOP), &frame).unwrap(),
                ArrayViewMut2::from_shape((1, DEEP_FILTER_HOP), &mut output).unwrap(),
            )
            .unwrap();
        rendered.extend_from_slice(&output);
    }
    let max_error = rendered
        .iter()
        .enumerate()
        .map(|(i, &actual)| {
            let expected = if i < delay { 0. } else { source[i - delay] };
            (actual - expected).abs()
        })
        .fold(0f32, f32::max);
    println!("DeepFilterNet2 unity-mask delay {delay}, maximum waveform error {max_error}");
    assert!(
        max_error < 1e-6,
        "reported delay does not match source audio: {max_error}"
    );
}
