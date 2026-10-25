//! Verify native neural arithmetic and independence from classical estimator history.

// Rust guideline compliant 2026-02-21
use crate::music_mask::{MusicMask, spectral_features};
use crate::{DenoiserPlugin, DenoiserPluginParams};
use sotf_host::{ParametricInPlacePlugin, ProcessContext};

const MODEL: &[u8] = include_bytes!("../../../models/music-noise-mask/model.synthetic.json");
const SHA: &str = "ab1a996543de22b77b38ea57363a57d23a65319161ba81f4ecce1d5b280fe0a1";

#[test]
fn music_features_and_network_match_frozen_numpy_reference() {
    let model = MusicMask::from_json(MODEL, SHA).unwrap();
    let rows: serde_json::Value = serde_json::from_slice(include_bytes!(
        "../../../models/music-noise-mask/parity.synthetic.json"
    ))
    .unwrap();
    let mut feature_error = 0.0_f64;
    let mut gain_error = 0.0_f64;
    for row in rows.as_array().unwrap() {
        let fft = row["fft_size"].as_u64().unwrap() as usize;
        let numbers = |key: &str| {
            row[key]
                .as_array()
                .unwrap()
                .iter()
                .map(|v| v.as_f64().unwrap())
                .collect::<Vec<_>>()
        };
        let power = numbers("power");
        let previous = numbers("previous");
        for bin in row["bins"].as_array().unwrap() {
            let k = bin["bin"].as_u64().unwrap() as usize;
            let f = spectral_features(&power, &previous, k, fft).unwrap();
            for (i, &v) in f.iter().enumerate() {
                feature_error = feature_error.max((v - bin["features"][i].as_f64().unwrap()).abs());
            }
            gain_error = gain_error.max(
                (f64::from(model.predict(&f).unwrap()) - bin["prediction"].as_f64().unwrap()).abs(),
            );
        }
    }
    assert!(feature_error < 1e-12, "{feature_error}");
    assert!(gain_error < 3e-8, "{gain_error}");
    println!("music_mask_parity feature_max={feature_error} gain_max={gain_error}");
    assert!(MusicMask::from_json(MODEL, &"0".repeat(64)).is_err());
    assert!(model.predict(&[f64::NAN; 6]).is_none());
}

#[test]
fn selected_learned_gains_never_feed_native_estimator_or_gain_history() {
    for low_latency in [false, true] {
        for mode in 0..4 {
            let params = DenoiserPluginParams {
                low_latency,
                reduction_db: 20.0,
                multi_resolution: mode == 1,
                polyphonic_detection: mode == 2,
                harmonic_percussive: mode == 3,
                spatial_denoise: mode == 3,
                ..Default::default()
            };
            let mut classical = DenoiserPlugin::from_params(2, params.clone());
            let mut learned = DenoiserPlugin::from_params(2, params);
            classical.initialize(48000.0).unwrap();
            learned.initialize(48000.0).unwrap();
            learned
                .install_qa_music_mask(Some(MusicMask::from_json(MODEL, SHA).unwrap()))
                .unwrap();
            learned.set_qa_music_mask_enabled(true).unwrap();
            let source: Vec<_> = (0..20002)
                .map(|i| ((i * 13 % 97) as f32 - 48.0) / 1024.0)
                .collect();
            let mut different = false;
            for block in source.chunks(386) {
                let mut a = block.to_vec();
                let mut b = a.clone();
                let context = ProcessContext::new(48000, block.len() / 2);
                classical.process_in_place(&mut a, &context).unwrap();
                learned.process_in_place(&mut b, &context).unwrap();
                different |= a != b;
                assert_eq!(classical.mcra.noise_psd, learned.mcra.noise_psd);
                assert_eq!(classical.mcra.smoothed_psd, learned.mcra.smoothed_psd);
                assert_eq!(classical.mcra.min_psd, learned.mcra.min_psd);
                assert_eq!(classical.mcra.min_psd_b, learned.mcra.min_psd_b);
                assert_eq!(classical.mcra.speech_presence, learned.mcra.speech_presence);
                assert_eq!(classical.mcra.frame_counter, learned.mcra.frame_counter);
                assert_eq!(classical.mcra.learning_active, learned.mcra.learning_active);
                assert_eq!(classical.gains.gain, learned.gains.gain);
                assert_eq!(classical.gains.smoothed_gain, learned.gains.smoothed_gain);
                assert_eq!(
                    classical.decision_directed.prev_power,
                    learned.decision_directed.prev_power
                );
            }
            assert!(different);
        }
    }
}
