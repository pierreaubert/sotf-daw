// Rust guideline compliant 2026-02-21
use super::{BeamformerPlugin, Plugin, ProcessContext};
use crate::BeamformerPluginParams;
use sotf_host::TailLength;

#[test]
fn ordinary_zero_continuation_keeps_learning_without_extending_audio_support() {
    for algorithm in [0, 2] {
        let mut plugin = BeamformerPlugin::from_params(
            48_000,
            BeamformerPluginParams {
                beamformer_type: algorithm,
                steer_angle_deg: 37.0,
                mic_spacing_cm: 50.0,
                ..Default::default()
            },
        )
        .unwrap();
        plugin.initialize(48_000.0).unwrap();
        let input: Vec<f32> = (0..1043)
            .flat_map(|n| {
                [
                    (n as f32 * 0.13).sin() * 0.25,
                    (n as f32 * 0.31).cos() * 0.125,
                ]
            })
            .collect();
        let mut output = vec![0.0; 1043];
        plugin
            .process(&input, &mut output, &ProcessContext::new(48_000, 1043))
            .unwrap();
        let covariance = plugin.mvdr.noise_cov_snapshot().to_vec();
        let weights = plugin.gsc.learned_snapshot();
        let TailLength::Finite(bound) = plugin.tail_length() else {
            panic!("finite prepared bound")
        };
        let frames = bound as usize + 1024;
        let silence = vec![0.0; frames * 2];
        let mut output = vec![f32::NAN; frames];
        plugin
            .process(&silence, &mut output, &ProcessContext::new(48_000, frames))
            .unwrap();
        assert!(output[bound as usize..].iter().all(|&x| x == 0.0));
        assert_eq!(plugin.tail_length(), TailLength::Finite(bound));
        if algorithm == 0 {
            assert_ne!(
                plugin.mvdr.noise_cov_snapshot(),
                covariance,
                "ordinary continuation must keep covariance adaptation enabled"
            );
        } else {
            let after = plugin.gsc.learned_snapshot();
            assert_ne!(
                after, weights,
                "retained references must continue adaptive updates"
            );
            plugin
                .process(&silence, &mut output, &ProcessContext::new(48_000, frames))
                .unwrap();
            assert_eq!(plugin.gsc.learned_snapshot(), after);
            assert!(output.iter().all(|&x| x == 0.0));
        }
    }
}
