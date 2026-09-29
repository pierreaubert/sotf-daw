// Rust guideline compliant 2026-02-21
use sotf_host::{Plugin, ProcessContext};
use sotf_plugin_beamformer::{BeamformerPlugin, BeamformerPluginParams};

const RATE: u32 = 48_000;

fn plugin(mics: usize, algorithm: usize) -> BeamformerPlugin {
    let mut plugin = BeamformerPlugin::from_params(
        RATE,
        BeamformerPluginParams {
            num_mics: mics,
            beamformer_type: algorithm,
            ..Default::default()
        },
    )
    .unwrap();
    plugin.initialize(RATE).unwrap();
    plugin
}

fn process(plugin: &mut BeamformerPlugin, input: &[f32]) -> Vec<f32> {
    let frames = input.len() / plugin.input_channels();
    let mut output = vec![f32::NAN; frames];
    assert_eq!(
        plugin.process(input, &mut output, &ProcessContext::new(RATE, frames)),
        Ok(frames)
    );
    output
}

#[test]
fn gsc_finite_extreme_input_recovers_without_reset() {
    for amplitude in [0.25, 1e-20, f32::MAX * 0.5, f32::MAX * 0.75, f32::MAX] {
        let mut plugin = plugin(4, 2);
        let hot = [amplitude, -amplitude, -amplitude, -amplitude];
        assert!(hot.iter().all(|x| x.is_finite()));
        let first = process(&mut plugin, &hot);
        assert!(first[0].is_finite(), "amplitude={amplitude}");
        // Broadside fixed sum is -A/2; initial adaptive weights are zero.
        assert!((f64::from(first[0]) / f64::from(amplitude) + 0.5).abs() < 1e-7);
        let silence = process(&mut plugin, &[0.0; 4096 * 4]);
        assert!(silence.iter().all(|x| x.is_finite()));
        assert!(silence[31..].iter().all(|&x| x == 0.0));
        let small: Vec<f32> = (0..257)
            .flat_map(|i| {
                let v = (i as f32 * 0.13).sin() * 1e-4;
                [v, -v, v * 0.5, v * 0.5]
            })
            .collect();
        let recovery = process(&mut plugin, &small);
        assert!(recovery.iter().all(|x| x.is_finite()));
        assert!(recovery.iter().any(|&x| x != 0.0));
        plugin.reset();
        let reset = process(&mut plugin, &small);
        assert!(reset.iter().all(|x| x.is_finite()));
    }
}

#[test]
fn mvdr_prolonged_silence_is_exact_zero_before_and_after_reset() {
    // The original f32 normalization overflows after approximately 850 hops
    // of quiet covariance decay, then recovers at the Cholesky floor.
    for mics in [2, 4, 8] {
        let mut plugin = plugin(mics, 0);
        let input = vec![0.0; 997 * mics];
        let mut output = vec![f32::NAN; 997];
        for epoch in 0..2 {
            plugin.reset();
            let mut position = 0;
            let mut iteration = 0;
            while position < 1100 * 256 {
                let frames = [17, 997, 256, 73][iteration % 4].min(1100 * 256 - position);
                assert_eq!(
                    plugin.process(
                        &input[..frames * mics],
                        &mut output[..frames],
                        &ProcessContext::new(RATE, frames),
                    ),
                    Ok(frames)
                );
                assert!(
                    output[..frames].iter().all(|&x| x == 0.0),
                    "mics={mics}, epoch={epoch}, frame={position}"
                );
                position += frames;
                iteration += 1;
            }
        }
    }
}
