//! XTC's sample-clock AutoGain must honor its smoothing control on real wet audio.
// Rust guideline compliant 2026-02-21
use sotf_host::{ParameterId, ParameterValue, Plugin, ProcessContext};
use sotf_plugin_xtc::{XtcPlugin, XtcPluginParams};

fn plugin(rate: u32, smoothing: f32, enabled: bool) -> XtcPlugin {
    let mut plugin = XtcPlugin::new(
        XtcPluginParams {
            fft_size: 1024,
            spectral_normalization: false,
            bypass_spectral_normalization: true,
            auto_gain_enabled: enabled,
            auto_gain_max_db: 12.0,
            ..Default::default()
        },
        rate,
    )
    .unwrap();
    plugin.initialize(f64::from(rate)).unwrap();
    let id = ParameterId::from("auto_gain_smoothing_ms");
    plugin
        .set_parameter(id.clone(), ParameterValue::Float(smoothing))
        .unwrap();
    assert_eq!(
        plugin.get_parameter(&id),
        Some(ParameterValue::Float(smoothing))
    );
    plugin
}

fn render(plugin: &mut XtcPlugin, input: &[f32], rate: u32, block: usize) -> Vec<f32> {
    let mut output = vec![0.0; input.len()];
    for (source, destination) in input.chunks(block * 2).zip(output.chunks_mut(block * 2)) {
        let frames = source.len() / 2;
        assert_eq!(
            plugin
                .process(source, destination, &ProcessContext::new(rate, frames))
                .unwrap(),
            frames
        );
    }
    output
}

fn energy(samples: &[f32]) -> f64 {
    samples.iter().map(|&x| f64::from(x).powi(2)).sum()
}

#[test]
fn smoothing_changes_xtc_audio_toward_independently_measured_level_and_resets() {
    for rate in [48_000, 96_000] {
        // Antipolarity excites the nonneutral difference transfer at 1 kHz.
        let polarity = -1.0;
        let input: Vec<_> = (0..rate as usize * 2)
            .flat_map(|n| {
                let x = (0.002
                    * (std::f64::consts::TAU * 1000.0 * n as f64 / f64::from(rate)).sin())
                    as f32;
                [x, polarity * x]
            })
            .collect();
        for block in [137, 512] {
            let mut audio = Vec::new();
            for smoothing in [25.0, 500.0] {
                let mut processor = plugin(rate, smoothing, true);
                let first = render(&mut processor, &input, rate, block);
                processor.reset();
                assert!(
                    first == render(&mut processor, &input, rate, block),
                    "reset changed audio"
                );
                audio.push(first);
            }
            let raw = render(&mut plugin(rate, 25.0, false), &input, rate, block);
            assert_eq!(
                raw,
                render(&mut plugin(rate, 500.0, false), &input, rate, block)
            );
            assert!(
                raw.iter().all(|x| x.abs() < 0.1),
                "quiet fixture must not engage the output limiter"
            );
            // A stationary single tone has the same loudness weighting before
            // and after this linear filter. The independently measured energy
            // ratio therefore determines compensation's required direction.
            let range = rate as usize..rate as usize * 3;
            let source_energy = energy(&input[range.clone()]);
            let raw_energy = energy(&raw[range.clone()]);
            let correction = (source_energy / raw_energy).ln();
            assert!(
                correction.abs() > 0.1,
                "fixture must require real compensation: rate={rate}, polarity={polarity}, correction={correction}"
            );
            let fast = energy(&audio[0][range.clone()]);
            let slow = energy(&audio[1][range]);
            assert!(
                correction.signum() * (fast / slow).ln() > 0.001,
                "rate={rate}, block={block}, polarity={polarity}, correction={correction}, fast={fast}, slow={slow}"
            );
            assert!(correction.signum() * (fast / raw_energy).ln() > 0.01);
        }
    }
}
