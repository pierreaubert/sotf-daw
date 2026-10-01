//! AUD145 measures Warped EQ audio against independent documented responses.

// Rust guideline compliant 2026-02-21

use math_audio_iir_fir::bark_lambda;
use sotf_host::{AutoGainParams, ParameterId, ParameterValue, ParametricPlugin, ProcessContext};
use sotf_plugin_eq::{
    BiquadFilterConfig, EqBandPlacement, EqFilterTopology, EqPlugin, EqPluginParams,
};
use std::fs;
use std::path::PathBuf;

const RATES: [(u32, f64); 3] = [
    (44_100, 0.756_413_523_284_413_6),
    (48_000, 0.766_017_000_483_164_6),
    (96_000, 0.821_076_462_925_666_8),
];
const FRAMES_PER_BLOCK: usize = 193;
const IMPULSE_FRAMES: usize = 16_384;
const TONE_FREQUENCY_HZ: f64 = 1_379.0;
const TONE_GAIN_DB: f64 = 7.0;

fn warped_filter(lambda: Option<f64>) -> BiquadFilterConfig {
    BiquadFilterConfig {
        filter_type: "peak".into(),
        freq: TONE_FREQUENCY_HZ,
        q: 0.83,
        db_gain: TONE_GAIN_DB,
        order: 2,
        topology: EqFilterTopology::WarpedBiquad,
        placement: Some(EqBandPlacement::Stereo),
        lambda,
        kautz_sections: Vec::new(),
    }
}

fn public_warped_plugin(sample_rate: u32, lambda: Option<f64>) -> EqPlugin {
    let mut plugin = EqPlugin::from_params(
        1,
        sample_rate,
        EqPluginParams {
            filters: vec![warped_filter(lambda)],
            channel_filters: None,
            stereo_pairs: None,
            auto_gain: AutoGainParams::default(),
        },
    )
    .expect("construct public one-channel Warped EQ");
    plugin
        .parametric_set_parameter(
            ParameterId::from("auto_gain_enabled"),
            ParameterValue::Bool(false),
        )
        .expect("disable whole-plugin AutoGain for filter response");
    plugin
        .plugin_initialize(sample_rate)
        .expect("initialize public Warped EQ");
    plugin
}

#[test]
fn bark_lambda_uses_kilohertz_sample_rate_units() {
    for (sample_rate, expected) in RATES {
        let actual = bark_lambda(f64::from(sample_rate));
        assert!(
            (actual - expected).abs() <= 1.0e-12,
            "rate={sample_rate}: expected λ={expected:.15}, got {actual:.15}"
        );
    }
}

#[test]
fn public_audio_peak_reaches_requested_frequency_and_gain() {
    let sample_rate = 48_000_u32;
    let mut plugin = public_warped_plugin(sample_rate, Some(0.37));
    let warmup_frames = sample_rate as usize;
    let measure_frames = sample_rate as usize;
    let total_frames = warmup_frames + measure_frames;
    let omega = 2.0 * std::f64::consts::PI * TONE_FREQUENCY_HZ / f64::from(sample_rate);
    let mut input = vec![0.0_f32; FRAMES_PER_BLOCK];
    let mut output = vec![0.0_f32; FRAMES_PER_BLOCK];
    let mut in_phase = 0.0_f64;
    let mut quadrature = 0.0_f64;

    for start in (0..total_frames).step_by(FRAMES_PER_BLOCK) {
        let frames = (total_frames - start).min(FRAMES_PER_BLOCK);
        for (offset, sample) in input[..frames].iter_mut().enumerate() {
            let absolute_frame = start + offset;
            *sample = (omega * absolute_frame as f64).sin() as f32;
        }
        let processed = plugin
            .process(
                &input[..frames],
                &mut output[..frames],
                &ProcessContext::new(sample_rate, frames),
            )
            .expect("process full public EQ callback");
        assert_eq!(processed, frames, "public EQ returned a short callback");

        for (offset, &sample) in output[..frames].iter().enumerate() {
            let absolute_frame = start + offset;
            if absolute_frame >= warmup_frames {
                let phase = omega * absolute_frame as f64;
                in_phase += f64::from(sample) * phase.sin();
                quadrature += f64::from(sample) * phase.cos();
            }
        }
    }

    let amplitude = 2.0 / measure_frames as f64 * in_phase.hypot(quadrature);
    let measured_db = 20.0 * amplitude.log10();
    assert!(
        (measured_db - TONE_GAIN_DB).abs() <= 0.05,
        "requested center gain is {TONE_GAIN_DB:.3} dB, measured {measured_db:.6} dB"
    );
}

/// Capture public impulse responses for the independent f64 reference.
#[test]
#[ignore = "explicit independent Warped response capture; set AUD145_WARPED_CAPTURE_DIR"]
fn capture_public_warped_impulse_matrix() {
    let capture_dir = PathBuf::from(
        std::env::var_os("AUD145_WARPED_CAPTURE_DIR")
            .expect("set AUD145_WARPED_CAPTURE_DIR to a new capture directory"),
    );
    fs::create_dir_all(&capture_dir).expect("create Warped capture directory");

    for (sample_rate, _) in RATES {
        for (lambda_name, lambda) in [
            ("zero", Some(0.0)),
            ("explicit", Some(0.37)),
            ("auto_bark", None),
        ] {
            let mut plugin = public_warped_plugin(sample_rate, lambda);
            let mut input = vec![0.0_f32; IMPULSE_FRAMES];
            input[0] = 1.0;
            let mut output = vec![0.0_f32; IMPULSE_FRAMES];

            for start in (0..IMPULSE_FRAMES).step_by(FRAMES_PER_BLOCK) {
                let frames = (IMPULSE_FRAMES - start).min(FRAMES_PER_BLOCK);
                let processed = plugin
                    .process(
                        &input[start..start + frames],
                        &mut output[start..start + frames],
                        &ProcessContext::new(sample_rate, frames),
                    )
                    .expect("process public Warped impulse callback");
                assert_eq!(processed, frames, "public EQ returned a short callback");
            }

            assert!(output.iter().all(|sample| sample.is_finite()));
            assert!(output.iter().any(|sample| sample.abs() > 1.0e-6));
            let path = capture_dir.join(format!("warped-{sample_rate}-{lambda_name}.f32"));
            let mut bytes = Vec::with_capacity(output.len() * std::mem::size_of::<f32>());
            for sample in &output {
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
            fs::write(path, bytes).expect("write public EQ impulse response");
        }
    }
}
