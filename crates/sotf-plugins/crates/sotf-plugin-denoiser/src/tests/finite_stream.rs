//! Explicit capture is a measurement and must not ingest synthetic EOF padding.
// Rust guideline compliant 2026-02-21
use crate::{DenoiserPlugin, DenoiserPluginParams};
use sotf_host::{ParametricInPlacePlugin, ProcessContext};
#[test]
fn eos_freezes_capture_storage_and_profile_while_adaptive_audio_continues() {
    for low in [false, true] {
        for multi in [false, true] {
            for pnd in [false, true] {
                let make = || {
                    let mut p = DenoiserPlugin::from_params(
                        2,
                        DenoiserPluginParams {
                            low_latency: low,
                            multi_resolution: multi,
                            polyphonic_detection: pnd,
                            ..Default::default()
                        },
                    );
                    p.initialize(48000.0).unwrap();
                    p.noise_profile.has_noise_profile = true;
                    p.noise_profile.use_captured_profile = true;
                    for ch in &mut p.noise_profile.noise_profile_storage {
                        ch.fill(0.125);
                    }
                    p.noise_profile.learning_frames_target = 2;
                    p.start_learning();
                    p
                };
                let mut p = make();
                let mut reference = make();
                let n = p.config.fft_size;
                let hop = p.config.hop_size;
                let t = hop + 13;
                let mut a = vec![0.125; t * 2];
                let mut b = a.clone();
                p.process_in_place(&mut a, &ProcessContext::new(48000, t))
                    .unwrap();
                reference
                    .process_in_place(&mut b, &ProcessContext::new(48000, t))
                    .unwrap();
                assert_eq!(a, b);
                assert_eq!(p.noise_profile.learning_frames_count, 1);
                let captured = p.noise_profile.noise_profile_storage.clone();
                let partial = p.noise_profile.learning_accumulator.clone();
                let target = p.noise_profile.learning_frames_target;
                // This twin explicitly suppresses the same measurement step while
                // ordinary zero-input adaptive/filter processing continues unchanged.
                reference.noise_profile.is_learning = false;
                let frames = 2 * n + ((t - 1) / hop) * hop - t;
                let mut expected = vec![0.; frames * 2];
                for chunk in expected.chunks_mut(hop * 2) {
                    let count = chunk.len() / 2;
                    reference
                        .process_in_place(chunk, &ProcessContext::new(48000, count))
                        .unwrap();
                }
                let mut actual = Vec::new();
                loop {
                    let mut output = [0.; 14];
                    let s = p
                        .drain(&mut output, &ProcessContext::new(48000, 7))
                        .unwrap();
                    actual.extend_from_slice(&output[..s.frames * 2]);
                    if s.complete {
                        break;
                    }
                }
                assert_eq!(actual, expected);
                assert_eq!(p.noise_profile.learning_frames_count, 1);
                assert_eq!(p.noise_profile.learning_frames_target, target);
                assert_eq!(p.noise_profile.learning_accumulator, partial);
                assert_eq!(p.noise_profile.noise_profile_storage, captured);
                assert!(
                    p.noise_profile.is_learning
                        && p.noise_profile.has_noise_profile
                        && p.noise_profile.use_captured_profile
                );
                p.reset();
                assert!(!p.noise_profile.is_learning);
                assert_eq!(p.noise_profile.learning_frames_count, 0);
                assert!(
                    p.noise_profile
                        .learning_accumulator
                        .iter()
                        .flatten()
                        .all(|&v| v == 0.)
                );
                assert_eq!(p.noise_profile.noise_profile_storage, captured);
                assert!(p.noise_profile.has_noise_profile && p.noise_profile.use_captured_profile);
            }
        }
    }
}
